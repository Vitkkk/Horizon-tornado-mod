// What a storm contains at a point: hydrometeors (rain, hail, snow) for the radar and the rain
// curtains, and storm-scale winds (inflow, updraft, mesocyclone, outflow, downbursts) for the
// radar, the particles and straight-line wind damage.
//
// Positions are world coordinates (blocks); heights are physical metres above the ground, as
// the radar beam sees them. Contents are in g/m^3.

use super::{Storm, StormKind, SupercellType};
use crate::atmosphere::Environment;
use crate::math::{Vec3, smoothstep};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hydrometeors {
    pub rain: f64,
    pub hail: f64,
    pub snow: f64,
}

impl Hydrometeors {
    pub fn total(&self) -> f64 {
        self.rain + self.hail + self.snow
    }
    pub fn add(&mut self, o: Hydrometeors) {
        self.rain += o.rain;
        self.hail += o.hail;
        self.snow += o.snow;
    }
}

/// Height of the freezing level (m) for the vertical profiles.
pub const FREEZING_LEVEL: f64 = 4200.0;

#[inline]
fn gauss2(d2: f64, sigma: f64) -> f64 {
    (-d2 / (2.0 * sigma * sigma)).exp()
}

/// Splits a liquid-equivalent content into rain below the freezing level and snow/graupel above.
fn phase_split(content: f64, h: f64, top: f64) -> Hydrometeors {
    if h > top {
        return Hydrometeors::default();
    }
    let melt = smoothstep(FREEZING_LEVEL - 300.0, FREEZING_LEVEL + 300.0, h);
    let aloft = 1.0 - 0.6 * smoothstep(FREEZING_LEVEL, top, h);
    Hydrometeors { rain: content * (1.0 - melt), hail: 0.0, snow: content * melt * aloft }
}

impl Storm {
    /// Storm-relative frame: forward (motion) and left unit vectors.
    fn frame(&self) -> (Vec3, Vec3) {
        let f = if self.motion.horizontal_length() > 0.1 { self.motion.horizontal().normalized() } else { Vec3::new(1.0, 0.0, 0.0) };
        (f, f.cyclonic_tangent())
    }

    /// Rough bounding radius of everything the storm contains (blocks).
    pub fn extent(&self) -> f64 {
        match self.kind {
            StormKind::Qlcs => self.line_half_length + 5.0 * self.updraft_radius,
            StormKind::Supercell(SupercellType::LowPrecip) => 8.0 * self.updraft_radius,
            _ => 5.5 * self.updraft_radius,
        }
    }

    /// Hydrometeor content at world `at` (horizontal) and physical height `h`.
    pub fn hydrometeors(&self, at: Vec3, h: f64) -> Hydrometeors {
        let d = (at - self.pos).horizontal();
        if d.length_sq() > self.extent().powi(2) || self.precip <= 0.001 {
            return Hydrometeors::default();
        }
        let ru = self.updraft_radius.max(1.0);
        let (fwd, left) = self.frame();
        let mut out = Hydrometeors::default();
        match self.kind {
            StormKind::Supercell(t) => {
                let (peak, along, offset) = match t {
                    SupercellType::HighPrecip => (6.0, 1.4, 1.3),
                    SupercellType::Classic => (4.0, 1.5, 1.6),
                    SupercellType::LowPrecip => (1.2, 2.6, 2.6),
                };
                let q = peak * self.precip;
                // forward-flank core, carried downshear
                let c = self.downshear * (offset * ru) + left * (0.35 * ru);
                let rel = d - c;
                let a = rel.dot(self.downshear);
                let b = rel.dot(self.downshear.cyclonic_tangent());
                let core = gauss2(a * a, along * ru) * gauss2(b * b, 0.75 * ru);
                // anvil: light snow far downshear, aloft
                let anvil = gauss2((a - 2.5 * ru).powi(2), 3.5 * ru) * gauss2(b * b, 1.6 * ru) * smoothstep(7000.0, 9500.0, h);
                out.add(phase_split(q * core, h, self.top));
                out.snow += 0.3 * q * anvil;
                // hook echo wrapped around the mesocyclone's rear flank
                let m = self.meso.offset;
                let dm = d - m;
                let rm = dm.horizontal_length();
                if rm > 1e-3 && self.meso.strength > 0.1 {
                    let to_core = (c - m).normalized();
                    // angle measured cyclonically from the direction of the forward core
                    let ang = {
                        let x = dm.dot(to_core);
                        let y = dm.dot(to_core.cyclonic_tangent());
                        let mut ang = y.atan2(x);
                        if ang < 0.0 {
                            ang += std::f64::consts::TAU;
                        }
                        ang
                    };
                    let reach = std::f64::consts::PI * (0.55 + 0.7 * self.meso.occlusion + 0.8 * self.rain_wrap);
                    let along_hook = 1.0 - smoothstep(reach * 0.75, reach, ang);
                    let r_hook = self.meso.radius * 0.9;
                    let ring = gauss2((rm - r_hook).powi(2), 0.35 * r_hook);
                    let hook_q = q * (0.35 + 0.5 * self.rfd) * (if t == SupercellType::LowPrecip { 0.25 } else { 1.0 });
                    out.add(phase_split(hook_q * ring * along_hook * (1.0 - smoothstep(2500.0, 5000.0, h)), h, self.top));
                }
                // hail core beside the updraft, falling on its forward side
                let hc = self.downshear * (0.9 * ru) + left * (0.45 * ru);
                let hd = (d - hc).length_sq();
                let hail = self.hail * 3.0 * gauss2(hd, 0.55 * ru) * (1.0 - smoothstep(8000.0, self.top, h));
                out.hail += hail;
                // BWER: the updraft lifts precipitation out of the low and middle levels,
                // under an echo overhang aloft
                let du = d.length_sq();
                let in_updraft = gauss2(du, 0.6 * ru);
                let bwer = in_updraft * (1.0 - smoothstep(5000.0, 7500.0, h)) * smoothstep(8.0, 30.0, self.updraft);
                out.rain *= 1.0 - 0.95 * bwer;
                out.hail *= 1.0 - 0.9 * bwer;
                out.snow *= 1.0 - 0.8 * bwer;
                let overhang = in_updraft * smoothstep(6000.0, 7500.0, h) * (1.0 - smoothstep(10_000.0, self.top, h));
                out.snow += q * 0.6 * overhang;
            }
            StormKind::Single => {
                let q = 4.0 * self.precip;
                let core = gauss2(d.length_sq(), 0.9 * ru);
                out.add(phase_split(q * core, h, self.top));
                out.hail += self.hail * 1.5 * core;
            }
            StormKind::Multicell => {
                let q = 3.5 * self.precip;
                out.add(phase_split(q * gauss2(d.length_sq(), 0.9 * ru), h, self.top));
                for &(off, age) in &self.cells {
                    let life = smoothstep(300.0, 900.0, age) * (1.0 - smoothstep(1500.0, 2400.0, age));
                    let core = gauss2((d - off).length_sq(), 0.7 * ru) * life;
                    out.add(phase_split(q * core, h, self.top));
                    out.hail += self.hail * core;
                }
            }
            StormKind::Qlcs => {
                let s = d.dot(self.line_dir);
                let n = d.dot(fwd);
                let ends = 1.0 - smoothstep(self.line_half_length * 0.8, self.line_half_length, s.abs());
                // a bow: the middle of the line surges ahead
                let bow = 0.8 * ru * (1.0 - (s / self.line_half_length.max(1.0)).powi(2)).max(0.0);
                let lead = gauss2((n - bow).powi(2), 0.3 * ru);
                let strat = smoothstep(-5.0 * ru, -2.5 * ru, n) * (1.0 - smoothstep(-0.6 * ru, -0.2 * ru, n));
                let q = 5.0 * self.precip;
                out.add(phase_split(q * lead * ends, h, self.top));
                // trailing stratiform rain with a bright band near the melting level
                let bb = 1.0 + 1.5 * gauss2((h - FREEZING_LEVEL + 200.0).powi(2), 250.0);
                out.add(phase_split(0.7 * self.precip * strat * ends * bb, h, self.top * 0.8));
                out.hail += self.hail * 0.6 * lead * ends;
            }
        }
        out
    }

    /// Storm-scale wind (m/s, world vector) at world `at`, physical height `h`, excluding
    /// tornadoes and including the environment.
    pub fn wind(&self, env: &Environment, at: Vec3, h: f64) -> Vec3 {
        let mut v = env.wind(h);
        let d = (at - self.pos).horizontal();
        let dist2 = d.length_sq();
        let ru = self.updraft_radius.max(1.0);
        if dist2 > (self.extent() * 1.2).powi(2) {
            return v;
        }
        // updraft and its low-level inflow
        let r = dist2.sqrt();
        let top = self.top.max(1000.0);
        let w = self.updraft * gauss2(dist2, 0.6 * ru) * (std::f64::consts::PI * (h / top).clamp(0.0, 1.0)).sin();
        v.y += w;
        if h < 1500.0 && r > 1.0 {
            let inflow = -0.25 * self.updraft * (r / ru) * (1.0 - (r / ru).powi(2) / 4.0).max(0.0) * (1.0 - h / 1500.0);
            v += d / r * inflow;
        }
        // mesocyclone: a Rankine-like vortex, strongest a few km up, reaching the ground as it
        // strengthens
        if self.meso.strength > 0.02 {
            let dm = d - self.meso.offset;
            let rm = dm.horizontal_length();
            let rr = self.meso.radius.max(1.0);
            let profile = if rm < rr { rm / rr } else { (rr / rm).powf(0.8) };
            let vertical = smoothstep(0.0, 1500.0, h) * (1.0 - smoothstep(8000.0, top, h));
            let low = self.meso.strength.powf(0.7);
            let height_weight = (1.0 - vertical) * low + vertical;
            if rm > 1e-3 {
                v += (dm / rm).cyclonic_tangent() * (self.meso.v_rot * profile * height_weight * self.meso.strength.sqrt());
            }
        }
        // outflow from the precipitation cores near the ground (forward and rear flank)
        if h < 1000.0 {
            let shallow = 1.0 - h / 1000.0;
            let (fwd, left) = self.frame();
            let ffd_c = self.downshear * (1.4 * ru) + left * (0.3 * ru);
            v += radial_outflow(d - ffd_c, 1.5 * ru, 12.0 * self.ffd) * shallow;
            if self.rfd > 0.05 {
                let rfd_c = self.meso.offset - fwd * (0.6 * self.meso.radius) - left * (0.4 * self.meso.radius);
                v += radial_outflow(d - rfd_c, 0.9 * self.meso.radius.max(ru * 0.4), 22.0 * self.rfd) * shallow;
            }
        }
        for m in &self.microbursts {
            v += microburst_wind(m, at, h);
        }
        v
    }

    /// Straight-line wind gust near the ground (m/s) the storm itself produces at `at`, for the
    /// damage engine: outflow and downbursts, without the environment or tornadoes.
    pub fn surface_gust(&self, env: &Environment, at: Vec3) -> Vec3 {
        let h = 10.0;
        self.wind(env, at, h) - env.wind(h) - Vec3::new(0.0, self.wind(env, at, h).y, 0.0)
    }
}

/// Divergent outflow of peak `peak` at radius `radius` from the centre (offset `d`).
fn radial_outflow(d: Vec3, radius: f64, peak: f64) -> Vec3 {
    let r = d.horizontal_length();
    if r < 1e-3 || peak <= 0.0 {
        return Vec3::ZERO;
    }
    let s = r / radius;
    d / r * (peak * s * (1.0 - s * s).exp())
}

/// A microburst's wind: an outflow peaking at its radius and about 50 m up, over a downdraft.
pub fn microburst_wind(m: &super::Microburst, at: Vec3, h: f64) -> Vec3 {
    let d = (at - m.pos).horizontal();
    let r = d.horizontal_length();
    let e = m.envelope();
    if e <= 0.0 || r > m.radius * 4.0 {
        return Vec3::ZERO;
    }
    let s = r / m.radius;
    let vertical = (h.max(1.0) / 50.0).powf(0.25).min(1.2) * (-h / 800.0).exp();
    let out = if r > 1e-3 { d / r * (m.strength * s * ((1.0 - s * s) / 2.0).exp() * vertical) } else { Vec3::ZERO };
    let down = -m.strength * 0.6 * (-s * s).exp() * (h / 300.0).min(1.0);
    (out + Vec3::new(0.0, down, 0.0)) * e
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atmosphere::presets::Preset;
    use crate::config::WorldScale;

    fn mature(kind: StormKind, preset: Preset) -> (Storm, Environment) {
        let env = Environment::from_preset(preset);
        let mut s = Storm::new(1, kind, Vec3::ZERO, &env, &WorldScale::default(), 4);
        for _ in 0..2400 {
            s.step(1.0, &env, 0.0);
        }
        (s, env)
    }

    #[test]
    fn supercell_has_bwer_and_forward_core() {
        let (s, _) = mature(StormKind::Supercell(SupercellType::Classic), Preset::Outbreak);
        let ru = s.updraft_radius;
        let in_updraft_low = s.hydrometeors(s.pos, 2000.0).total();
        let core = s.hydrometeors(s.pos + s.downshear * (1.6 * ru), 1000.0).total();
        let overhang = s.hydrometeors(s.pos, 8000.0).total();
        assert!(core > 1.0, "core {core}");
        assert!(in_updraft_low < 0.2 * core, "bwer {in_updraft_low} vs {core}");
        assert!(overhang > in_updraft_low, "overhang {overhang}");
    }

    #[test]
    fn hp_wetter_than_lp() {
        let (hp, _) = mature(StormKind::Supercell(SupercellType::HighPrecip), Preset::HighPrecip);
        let (lp, _) = mature(StormKind::Supercell(SupercellType::LowPrecip), Preset::LowPrecip);
        let sample = |s: &Storm| -> f64 {
            let mut sum = 0.0;
            for i in -20..=20 {
                for j in -20..=20 {
                    let p = s.pos + Vec3::new(i as f64 * 200.0, 0.0, j as f64 * 200.0);
                    sum += s.hydrometeors(p, 800.0).rain;
                }
            }
            sum
        };
        assert!(sample(&hp) > 2.0 * sample(&lp));
    }

    #[test]
    fn mesocyclone_rotates_cyclonically() {
        let (s, env) = mature(StormKind::Supercell(SupercellType::Classic), Preset::Outbreak);
        let c = s.meso_center();
        let r = s.meso.radius;
        let east = s.wind(&env, c + Vec3::new(r, 0.0, 0.0), 3000.0) - env.wind(3000.0);
        let west = s.wind(&env, c - Vec3::new(r, 0.0, 0.0), 3000.0) - env.wind(3000.0);
        // north (-z) on the east side, south on the west
        assert!(east.z < -5.0 && west.z > 5.0, "{east:?} {west:?}");
    }

    #[test]
    fn microburst_diverges_at_the_ground() {
        let m = super::super::Microburst { pos: Vec3::ZERO, age: 120.0, life: 400.0, strength: 40.0, radius: 300.0 };
        let east = microburst_wind(&m, Vec3::new(300.0, 0.0, 0.0), 50.0);
        let west = microburst_wind(&m, Vec3::new(-300.0, 0.0, 0.0), 50.0);
        assert!(east.x > 25.0 && west.x < -25.0, "{east:?}");
        assert!(microburst_wind(&m, Vec3::ZERO, 500.0).y < -10.0);
    }
}
