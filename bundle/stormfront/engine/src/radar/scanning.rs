// Radar geometry and scanning.
//
// The beam rises with range over a curved earth (4/3 effective radius) and widens with it
// (about 1 degree), so far storms are sampled high and coarsely: a mesocyclone stays visible
// far out, a tornado's couplet only nearby, and the lowest levels of distant storms not at all.
// Each gate averages several sub-samples across the beam (power-weighted), which is what
// smears small vortices with range. Reflectivity is attenuated along the radial (C and X band),
// echoes under the range-dependent noise floor are lost, and noisy gates get noisy velocity and
// a low-biased CC. Sweeps go through a volume coverage pattern of rising elevation angles.
//
// Horizontal distances are world blocks; the beam's height uses physical range (world range
// divided by `storm_length`), matching how storm-scale geometry is compressed (config.rs).

use super::correlation::{correlation, debris_like};
use super::reflectivity::{Scatter, from_dbz, noise_floor, scatter_at, to_dbz};
use super::velocity::{Couplet, air_motion, alias, radial_velocity};
use crate::math::{Rng, Vec3};
use crate::storms::system::StormSystem;

pub const EARTH_RADIUS: f64 = 6_371_000.0;
pub const K_EFFECTIVE: f64 = 4.0 / 3.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    S,
    C,
    X,
}

impl Band {
    /// One-way specific attenuation (dB/km) for linear reflectivity `z` in rain.
    pub fn attenuation(self, z: f64) -> f64 {
        let a = match self {
            Band::S => 3.0e-6,
            Band::C => 1.6e-5,
            Band::X => 9.0e-5,
        };
        a * z.powf(0.72)
    }
    pub fn name(self) -> &'static str {
        match self {
            Band::S => "S",
            Band::C => "C",
            Band::X => "X",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Product {
    Reflectivity,
    Velocity,
    Correlation,
}

impl Product {
    pub fn parse(s: &str) -> Option<Product> {
        Some(match s.to_ascii_lowercase().as_str() {
            "reflectivity" | "ref" | "refletividade" | "dbz" | "z" => Product::Reflectivity,
            "velocity" | "vel" | "velocidade" | "doppler" | "v" => Product::Velocity,
            "cc" | "correlation" | "rhohv" | "correlacao" | "correlação" => Product::Correlation,
            _ => return None,
        })
    }
    pub fn short(self) -> &'static str {
        match self {
            Product::Reflectivity => "REF",
            Product::Velocity => "VEL",
            Product::Correlation => "CC",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RadarSite {
    /// Antenna position (world).
    pub pos: Vec3,
    pub band: Band,
    /// Beam width (degrees).
    pub beamwidth: f64,
    /// Nyquist velocity (m/s); 0 disables aliasing.
    pub nyquist: f64,
    /// Range (world blocks) and gate length (world blocks).
    pub max_range: f64,
    pub gate: f64,
    /// Elevation angles (degrees) of the volume coverage pattern.
    pub vcp: Vec<f64>,
    pub aliasing: bool,
    /// Physical metres per world block for storm-scale geometry (1 / storm_length).
    pub scale: f64,
}

impl RadarSite {
    pub fn new(pos: Vec3, scale: f64) -> Self {
        RadarSite {
            pos,
            band: Band::S,
            beamwidth: 0.95,
            nyquist: 27.0,
            max_range: 40_000.0,
            gate: 250.0,
            vcp: vec![0.5, 0.9, 1.3, 1.8, 2.4, 3.1, 4.0, 5.1, 6.4],
            aliasing: false,
            scale,
        }
    }
}

/// Height (m) of the beam centre above the antenna at slant range `r` (m), elevation `el` (rad).
pub fn beam_height(r: f64, el: f64) -> f64 {
    let ke = K_EFFECTIVE * EARTH_RADIUS;
    (r * r + ke * ke + 2.0 * r * ke * el.sin()).sqrt() - ke
}

/// Unit vector of a beam at azimuth `az` (rad, 0 north, clockwise) and elevation `el` (rad).
pub fn beam_dir(az: f64, el: f64) -> Vec3 {
    Vec3::new(el.cos() * az.sin(), el.sin(), -el.cos() * az.cos())
}

/// One elevation scan.
#[derive(Clone, Debug, PartialEq)]
pub struct Sweep {
    pub elevation: f64,
    pub radials: usize,
    pub gates: usize,
    pub gate: f64,
    pub site: Vec3,
    /// Game tick the sweep finished.
    pub tick: u64,
    /// dBZ, m/s and CC per (radial, gate); NaN where there is no echo.
    pub dbz: Vec<f32>,
    pub vel: Vec<f32>,
    pub cc: Vec<f32>,
    /// Rotation signatures and debris signatures found.
    pub couplets: Vec<Couplet>,
    pub tds: Vec<Vec3>,
    pub nyquist: f64,
}

impl Sweep {
    pub fn new(site: &RadarSite, elevation: f64, radials: usize, gates: usize) -> Self {
        let n = radials * gates;
        Sweep {
            elevation,
            radials,
            gates,
            gate: site.max_range / gates as f64,
            site: site.pos,
            tick: 0,
            dbz: vec![f32::NAN; n],
            vel: vec![f32::NAN; n],
            cc: vec![f32::NAN; n],
            couplets: Vec::new(),
            tds: Vec::new(),
            nyquist: if site.aliasing { site.nyquist } else { 0.0 },
        }
    }
    #[inline]
    pub fn idx(&self, radial: usize, gate: usize) -> usize {
        radial * self.gates + gate
    }
    /// The gate holding world point `at`, if inside the sweep.
    pub fn locate(&self, at: Vec3) -> Option<(usize, usize)> {
        let d = (at - self.site).horizontal();
        let r = d.length();
        let g = (r / self.gate) as usize;
        if g >= self.gates {
            return None;
        }
        let az = d.x.atan2(-d.z).rem_euclid(std::f64::consts::TAU);
        let a = ((az / std::f64::consts::TAU) * self.radials as f64) as usize % self.radials;
        Some((a, g))
    }
    pub fn value(&self, product: super::scanning::Product, at: Vec3) -> Option<f32> {
        let (a, g) = self.locate(at)?;
        let i = self.idx(a, g);
        let v = match product {
            Product::Reflectivity => self.dbz[i],
            Product::Velocity => self.vel[i],
            Product::Correlation => self.cc[i],
        };
        (!v.is_nan()).then_some(v)
    }
}

/// Sub-sample offsets across the beam, in beam widths, and their weights.
const PATTERN: [(f64, f64, f64); 5] = [(0.0, 0.0, 1.0), (-0.35, 0.0, 0.7), (0.35, 0.0, 0.7), (0.0, -0.35, 0.7), (0.0, 0.35, 0.7)];

/// Compute radials `from..to` of `sweep`, with `subsamples` per gate. True (unaliased)
/// velocities are kept in `truth` for the detection.
pub fn compute_radials(sys: &StormSystem, site: &RadarSite, sweep: &mut Sweep, truth: &mut [f32], from: usize, to: usize, subsamples: usize, rng: &mut Rng) {
    let bw = site.beamwidth.to_radians();
    let el0 = sweep.elevation.to_radians();
    let antenna = site.pos.y - sys.ground_y;
    let subs = &PATTERN[..subsamples.clamp(1, PATTERN.len())];
    // what can echo: storms and tornado debris clouds
    let targets: Vec<(Vec3, f64)> = sys
        .storms
        .iter()
        .map(|s| (s.pos, s.extent()))
        .chain(sys.tornadoes.iter().filter(|t| t.debris_load > 1.0).map(|t| (t.pos, (t.params.r_max * 8.0).max(300.0))))
        .collect();
    for a in from..to.min(sweep.radials) {
        let az0 = (a as f64 + 0.5) / sweep.radials as f64 * std::f64::consts::TAU;
        let mut pia = 0.0; // two-way path-integrated attenuation (dB)
        for g in 0..sweep.gates {
            let rw = (g as f64 + 0.5) * sweep.gate;
            let centre = site.pos + Vec3::new(az0.sin(), 0.0, -az0.cos()) * rw;
            // clear air: nothing to see
            let near = targets.iter().any(|(p, r)| (centre - *p).horizontal_length() < r + bw * rw + sweep.gate);
            let i = sweep.idx(a, g);
            if !near {
                continue;
            }
            let rp = rw * site.scale;
            let mut z = Scatter::default();
            let mut wsum = 0.0;
            let mut vnum = 0.0;
            let mut vden = 0.0;
            for &(daz, del, w) in subs {
                let az = az0 + daz * bw;
                let el = el0 + del * bw;
                let pos = site.pos + Vec3::new(az.sin(), 0.0, -az.cos()) * rw;
                let h = beam_height(rp, el) + antenna;
                if h < 0.0 {
                    continue;
                }
                let zs = scatter_at(sys, pos, h);
                let zt = zs.total();
                z.add_weighted(&zs, w);
                wsum += w;
                if zt > 0.0 {
                    let air = air_motion(sys, pos, h);
                    vnum += w * zt * radial_velocity(air, &zs, beam_dir(az, el));
                    vden += w * zt;
                }
            }
            if wsum <= 0.0 {
                continue;
            }
            let zmean = Scatter { rain: z.rain / wsum, snow: z.snow / wsum, hail: z.hail / wsum, debris: z.debris / wsum };
            let ztot = zmean.total();
            let dbz_true = to_dbz(ztot);
            let dbz = dbz_true - pia;
            let gate_km = sweep.gate * site.scale / 1000.0;
            pia += 2.0 * site.band.attenuation(from_dbz(dbz_true.min(60.0)) * (zmean.rain + zmean.hail) / ztot.max(1e-9)) * gate_km;
            let floor = noise_floor(rp);
            if ztot <= 0.0 || dbz < floor {
                continue;
            }
            let snr = dbz - floor;
            sweep.dbz[i] = (dbz + rng.normal() * 0.6) as f32;
            let v_true = if vden > 0.0 { vnum / vden } else { 0.0 };
            let v = v_true + rng.normal() * (0.6 + 5.0 / (1.0 + snr / 2.0));
            truth[i] = v as f32;
            sweep.vel[i] = alias(v, sweep.nyquist) as f32;
            let cc = correlation(&zmean) - 0.07 / (1.0 + snr / 3.0) + rng.normal() * (0.006 + 0.05 / (1.0 + snr / 4.0));
            sweep.cc[i] = cc.clamp(0.2, 1.05) as f32;
        }
    }
}

/// Find velocity couplets (mesocyclones, TVSs) and debris signatures in a finished sweep, from
/// its true velocities.
pub fn detect(sweep: &mut Sweep, truth: &[f32]) {
    let mut found: Vec<Couplet> = Vec::new();
    let n = sweep.radials;
    for g in 2..sweep.gates {
        let rw = (g as f64 + 0.5) * sweep.gate;
        let arc = rw * std::f64::consts::TAU / n as f64;
        for a in 0..n {
            let i0 = sweep.idx(a, g);
            let i1 = sweep.idx((a + 1) % n, g);
            let (v0, v1) = (truth[i0], truth[i1]);
            if v0.is_nan() || v1.is_nan() {
                continue;
            }
            let dv = (v1 - v0) as f64;
            if dv < 20.0 {
                continue;
            }
            // extremes within a few radials either side for the rotational velocity
            let span = ((1500.0 / arc).ceil() as usize).clamp(1, 12);
            let (mut vmin, mut vmax) = (f64::MAX, f64::MIN);
            let (mut amin, mut amax) = (a, a);
            for k in 0..=span {
                let al = (a + n - k) % n;
                let ar = (a + 1 + k) % n;
                let l = truth[sweep.idx(al, g)] as f64;
                let r = truth[sweep.idx(ar, g)] as f64;
                if !l.is_nan() && l < vmin {
                    vmin = l;
                    amin = al;
                }
                if !r.is_nan() && r > vmax {
                    vmax = r;
                    amax = ar;
                }
            }
            let v_rot = (vmax - vmin) / 2.0;
            let sep = ((amax + n - amin) % n) as f64 * arc;
            let az = (a as f64 + 1.0) / n as f64 * std::f64::consts::TAU;
            let pos = sweep.site + Vec3::new(az.sin(), 0.0, -az.cos()) * rw;
            let tvs = dv >= 32.0 && arc <= 900.0;
            if v_rot < 12.0 && !tvs {
                continue;
            }
            // keep the strongest per neighbourhood
            if let Some(c) = found.iter_mut().find(|c| (c.pos - pos).horizontal_length() < 1200.0) {
                if dv > c.delta_v {
                    *c = Couplet { pos, delta_v: dv, v_rot: v_rot.max(c.v_rot), diameter: sep, tvs: tvs || c.tvs };
                }
            } else {
                found.push(Couplet { pos, delta_v: dv, v_rot, diameter: sep, tvs });
            }
        }
    }
    // debris signatures near rotation
    let mut tds = Vec::new();
    for c in &found {
        if c.delta_v < 25.0 {
            continue;
        }
        let Some((a, g)) = sweep.locate(c.pos) else { continue };
        let mut hits = 0;
        for da in 0..7 {
            for dg in 0..5 {
                let aa = (a + n + da - 3) % n;
                let gg = (g + dg).saturating_sub(2).min(sweep.gates - 1);
                let i = sweep.idx(aa, gg);
                if debris_like(sweep.dbz[i] as f64, sweep.cc[i] as f64) {
                    hits += 1;
                }
            }
        }
        if hits >= 2 {
            tds.push(c.pos);
        }
    }
    found.sort_by(|a, b| b.delta_v.total_cmp(&a.delta_v));
    found.truncate(12);
    sweep.couplets = found;
    sweep.tds = tds;
}

/// Compute a whole sweep at once (used by the background worker and by tests).
pub fn compute_sweep(sys: &StormSystem, site: &RadarSite, elevation: f64, radials: usize, gates: usize, subsamples: usize, seed: u64) -> Sweep {
    let mut sweep = Sweep::new(site, elevation, radials, gates);
    let mut truth = vec![f32::NAN; radials * gates];
    let mut rng = Rng::new(seed);
    compute_radials(sys, site, &mut sweep, &mut truth, 0, radials, subsamples, &mut rng);
    detect(&mut sweep, &truth);
    sweep
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atmosphere::Environment;
    use crate::atmosphere::presets::Preset;
    use crate::config::WorldScale;
    use crate::storms::{StormKind, SupercellType};
    use crate::tornado::SpawnShape;

    #[test]
    fn beam_rises_with_range() {
        let h50 = beam_height(50_000.0, 0.5f64.to_radians());
        // about 0.6 km from the elevation plus 0.15 km from the earth's curvature
        assert!((550.0..800.0).contains(&h50), "{h50}");
        assert!(beam_height(150_000.0, 0.5f64.to_radians()) > 2500.0);
    }

    fn system_with_tornado(debris: f64) -> (StormSystem, Vec3) {
        let scale = WorldScale::default();
        let mut sys = StormSystem::new(Environment::from_preset(Preset::Outbreak), scale, 2);
        let at = Vec3::new(0.0, 64.0, -4000.0);
        sys.spawn_storm(StormKind::Supercell(SupercellType::Classic), at, 0.0);
        let id = sys.spawn_tornado(SpawnShape::Wedge, at);
        for _ in 0..200 {
            sys.step(0.05);
        }
        let t = sys.tornado_mut(id).unwrap();
        t.params.v_max = 85.0;
        t.params.r_max = 150.0;
        t.debris_load = debris;
        let pos = t.pos;
        (sys, pos)
    }

    #[test]
    fn tornado_produces_a_couplet_and_debris_only_with_debris() {
        let scale = WorldScale::default();
        let (sys, at) = system_with_tornado(20_000.0);
        let mut site = RadarSite::new(Vec3::new(0.0, 70.0, 0.0), 1.0 / scale.storm_length);
        site.max_range = 8000.0;
        let sweep = compute_sweep(&sys, &site, 0.5, 360, 160, 3, 1);
        assert!(sweep.couplets.iter().any(|c| (c.pos - at).horizontal_length() < 500.0 && c.delta_v > 30.0), "{:?}", sweep.couplets);
        assert!(!sweep.tds.is_empty(), "debris lofted: a TDS");
        let v = sweep.value(Product::Reflectivity, at).unwrap_or(f32::NAN);
        assert!(v > 30.0, "{v}");

        let (clean, _) = system_with_tornado(0.0);
        let sweep = compute_sweep(&clean, &site, 0.5, 360, 160, 3, 1);
        assert!(sweep.tds.is_empty(), "no debris, no TDS: {:?}", sweep.tds);
    }

    #[test]
    fn velocity_is_radial_not_total() {
        // a uniform wind blowing east, seen north of the radar: zero radial velocity
        let scale = WorldScale::default();
        let mut sys = StormSystem::new(Environment::from_preset(Preset::Pulse), scale, 3);
        sys.env.spec.hodograph = vec![(0.0, 20.0, 270.0), (12000.0, 20.0, 270.0)];
        let env = Environment::from_spec(sys.env.spec.clone());
        sys.set_environment(env);
        sys.spawn_storm(StormKind::Single, Vec3::new(0.0, 64.0, -3000.0), 0.0);
        let site = RadarSite::new(Vec3::new(0.0, 70.0, 0.0), 4.0);
        let sweep = compute_sweep(&sys, &site, 0.5, 360, 160, 1, 2);
        let north = sweep.value(Product::Velocity, Vec3::new(0.0, 0.0, -3000.0));
        if let Some(v) = north {
            assert!(v.abs() < 6.0, "north of the radar a westerly is crossbeam: {v}");
        }
        let (a, _) = sweep.locate(Vec3::new(0.0, 0.0, -3000.0)).unwrap();
        assert!(a == 0 || a == 359);
    }

    #[test]
    fn aliasing_folds_velocities() {
        let scale = WorldScale::default();
        let (sys, at) = system_with_tornado(0.0);
        let mut site = RadarSite::new(Vec3::new(0.0, 70.0, 0.0), 1.0 / scale.storm_length);
        site.max_range = 8000.0;
        site.aliasing = true;
        site.nyquist = 20.0;
        let sweep = compute_sweep(&sys, &site, 0.5, 360, 160, 1, 3);
        let max = sweep.vel.iter().filter(|v| !v.is_nan()).fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(max <= 20.0 + 1e-3, "{max}");
        // the detection still sees the true rotation
        assert!(sweep.couplets.iter().any(|c| (c.pos - at).horizontal_length() < 600.0));
    }
}
