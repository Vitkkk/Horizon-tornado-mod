// A tornado's life: formation, maturity with pulsing intensity and width, rope-out as its
// parent mesocyclone occludes, and dissipation. Its ground-relative wind is the vortex's
// own (axisymmetric plus a wavenumber-1 rear-flank asymmetry), the subvortices', gusts and the
// translation.

use super::condensation::{Funnel, FunnelInputs, Morphology, compute_funnel};
use super::multivortex::MultiVortex;
use super::vortex::{RadialProfile, VortexParams};
use crate::math::{OuProcess, Rng, Vec3, clamp01, lerp, smoothstep};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TornadoStage {
    #[default]
    Forming,
    Mature,
    /// The parent mesocyclone occludes: the vortex narrows, tilts and turns left.
    Shrinking,
    Dissipating,
    Dead,
}

impl TornadoStage {
    pub fn name(self) -> &'static str {
        match self {
            TornadoStage::Forming => "forming",
            TornadoStage::Mature => "mature",
            TornadoStage::Shrinking => "rope-out",
            TornadoStage::Dissipating => "dissipating",
            TornadoStage::Dead => "dissipated",
        }
    }
    pub fn id(self) -> u8 {
        self as u8
    }
    pub fn from_id(id: u8) -> Self {
        match id {
            1 => TornadoStage::Mature,
            2 => TornadoStage::Shrinking,
            3 => TornadoStage::Dissipating,
            4 => TornadoStage::Dead,
            _ => TornadoStage::Forming,
        }
    }
}

/// The appearance asked for by `/tornado spawn <shape>`: initial conditions that lead to it
/// (the funnel itself is still computed from the vortex and the cloud base).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnShape {
    Rope,
    Cone,
    Stovepipe,
    Wedge,
    ElephantTrunk,
    RainWrapped,
    MultipleVortex,
    Satellite,
}

impl SpawnShape {
    pub fn parse(s: &str) -> Option<SpawnShape> {
        Some(match s.to_ascii_lowercase().as_str() {
            "rope" | "corda" => SpawnShape::Rope,
            "cone" => SpawnShape::Cone,
            "stovepipe" | "chamine" | "chaminé" => SpawnShape::Stovepipe,
            "wedge" | "cunha" => SpawnShape::Wedge,
            "elephant" | "elephant_trunk" | "trunk" | "tromba" => SpawnShape::ElephantTrunk,
            "rainwrapped" | "rain_wrapped" | "rain-wrapped" | "hp" => SpawnShape::RainWrapped,
            "multivortex" | "multi" | "multiple" | "multiplos" => SpawnShape::MultipleVortex,
            "satellite" | "satelite" | "satélite" => SpawnShape::Satellite,
            _ => return None,
        })
    }
    pub const NAMES: &'static str = "rope|cone|stovepipe|wedge|elephant|rainwrapped|multivortex|satellite";

    /// (v_max, r_max, lcl m, multivortex, beta, tilt fraction, rain wrap, life s)
    fn setup(self) -> (f64, f64, f64, bool, f64, f64, f64, f64) {
        match self {
            SpawnShape::Rope => (48.0, 14.0, 500.0, false, 0.0, 0.45, 0.0, 240.0),
            SpawnShape::Cone => (62.0, 55.0, 750.0, false, 0.3, 0.08, 0.0, 600.0),
            SpawnShape::Stovepipe => (74.0, 85.0, 450.0, false, 0.6, 0.05, 0.0, 720.0),
            SpawnShape::Wedge => (88.0, 260.0, 420.0, false, 1.0, 0.04, 0.1, 1100.0),
            SpawnShape::ElephantTrunk => (55.0, 35.0, 700.0, false, 0.2, 0.42, 0.0, 420.0),
            SpawnShape::RainWrapped => (80.0, 180.0, 500.0, false, 0.9, 0.06, 0.85, 900.0),
            SpawnShape::MultipleVortex => (82.0, 150.0, 1400.0, true, 2.2, 0.06, 0.0, 900.0),
            SpawnShape::Satellite => (42.0, 18.0, 800.0, false, 0.0, 0.15, 0.0, 180.0),
        }
    }
}

/// Values a player forced with commands; they win over the storm's own.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Overrides {
    pub v_max: Option<f64>,
    pub speed: Option<f64>,
    pub multivortex: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct Tornado {
    pub id: u32,
    pub storm_id: Option<u32>,
    /// A satellite tornado orbits this primary tornado.
    pub satellite_of: Option<u32>,
    /// Ground centre (world coordinates; y is the ground height under it).
    pub pos: Vec3,
    /// Translation (m/s).
    pub velocity: Vec3,
    pub params: VortexParams,
    /// What the storm (or a command) drives the vortex towards.
    pub target_v_max: f64,
    pub target_r_max: f64,
    pub stage: TornadoStage,
    pub age: f64,
    pub life: f64,
    pub multivortex: MultiVortex,
    /// Axis displacement at the cloud base relative to the ground centre (blocks).
    pub tilt: Vec3,
    pub waviness: f64,
    pub wave_phase: f64,
    /// Physical LCL (m) and display cloud base (blocks above ground).
    pub lcl: f64,
    pub cloud_base: f64,
    pub rain_wrap: f64,
    /// Direction (rad) of the rear-flank side, where the asymmetric inflow strengthens winds.
    pub rfd_angle: f64,
    pub asymmetry: f64,
    /// Mass of debris lofted in the vortex (kg), fed by the damage engine; drives the radar's
    /// debris signature and the dark debris cloud.
    pub debris_load: f64,
    pub overrides: Overrides,
    pub funnel: Funnel,
    gust: OuProcess,
    width: OuProcess,
    rng: Rng,
}

/// What the parent storm tells its tornado each step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StormForcing {
    /// Storm motion (m/s).
    pub motion: Vec3,
    /// Mesocyclone strength 0..1 (low-level rotation).
    pub meso: f64,
    /// Whether the mesocyclone is occluding.
    pub occluding: bool,
    /// Environment's potential: peak v_max the environment supports (m/s).
    pub potential: f64,
    pub lcl: f64,
    pub cloud_base: f64,
    pub rain_wrap: f64,
    /// Where the mesocyclone is, for the tornado's drift towards it.
    pub meso_center: Vec3,
}

impl Tornado {
    pub fn new(id: u32, pos: Vec3, seed: u64) -> Self {
        let params = VortexParams::new(35.0, 20.0);
        Tornado {
            id,
            storm_id: None,
            satellite_of: None,
            pos,
            velocity: Vec3::ZERO,
            params,
            target_v_max: 60.0,
            target_r_max: 60.0,
            stage: TornadoStage::Forming,
            age: 0.0,
            life: 600.0,
            multivortex: MultiVortex::new(false),
            tilt: Vec3::ZERO,
            waviness: 0.1,
            wave_phase: 0.0,
            lcl: 800.0,
            cloud_base: 130.0,
            rain_wrap: 0.0,
            rfd_angle: 0.0,
            asymmetry: 0.08,
            debris_load: 0.0,
            overrides: Overrides::default(),
            funnel: Funnel::default(),
            gust: OuProcess::default(),
            width: OuProcess::default(),
            rng: Rng::new(seed ^ 0x7041_D0),
        }
    }

    /// A tornado set up to look like `shape`.
    pub fn spawn(id: u32, pos: Vec3, shape: SpawnShape, seed: u64) -> Self {
        let (v, r, lcl, multi, beta, tilt, rain, life) = shape.setup();
        let mut t = Tornado::new(id, pos, seed);
        t.target_v_max = v;
        t.target_r_max = r;
        t.params = VortexParams::new(v * 0.75, r * 0.6);
        t.params.beta = beta;
        if shape == SpawnShape::Rope || shape == SpawnShape::Satellite {
            t.params.profile = RadialProfile::ModifiedRankine { alpha: 0.7 };
        }
        t.lcl = lcl;
        t.multivortex = MultiVortex::new(multi);
        t.rain_wrap = rain;
        t.life = life;
        t.tilt = Vec3::new(tilt * t.cloud_base, 0.0, -tilt * 0.3 * t.cloud_base);
        t.waviness = if shape == SpawnShape::Rope { 0.7 } else { 0.15 };
        if shape == SpawnShape::Rope {
            t.stage = TornadoStage::Mature;
        }
        t
    }

    pub fn alive(&self) -> bool {
        self.stage != TornadoStage::Dead
    }

    /// Advance by `dt` seconds.
    pub fn step(&mut self, dt: f64, forcing: Option<&StormForcing>) {
        self.age += dt;
        let rng = &mut self.rng;
        let gust = self.gust.step(rng, dt, 12.0, 0.07);
        let width_noise = self.width.step(rng, dt, 35.0, 0.08);
        // the storm's demand
        let (mut demand_v, occluding) = match forcing {
            Some(f) => {
                self.lcl = lerp(self.lcl, f.lcl, clamp01(dt / 60.0));
                self.cloud_base = lerp(self.cloud_base, f.cloud_base, clamp01(dt / 30.0));
                self.rain_wrap = lerp(self.rain_wrap, f.rain_wrap.max(self.rain_wrap * 0.0), clamp01(dt / 40.0));
                (f.potential * (0.55 + 0.45 * f.meso), f.occluding)
            }
            None => (self.target_v_max, false),
        };
        if forcing.is_some() {
            demand_v = demand_v.min(self.target_v_max.max(demand_v * 0.8));
        }
        if let Some(v) = self.overrides.v_max {
            demand_v = v;
        }
        // stage changes
        let life_frac = self.age / self.life.max(1.0);
        match self.stage {
            TornadoStage::Forming if self.age > 45.0 || self.params.v_max > 0.85 * demand_v => {
                self.stage = TornadoStage::Mature;
            }
            TornadoStage::Mature if (occluding || life_frac > 0.72) && self.overrides.v_max.is_none() => {
                self.stage = TornadoStage::Shrinking;
            }
            TornadoStage::Shrinking if life_frac > 0.92 || self.params.r_max < 12.0 => {
                self.stage = TornadoStage::Dissipating;
            }
            TornadoStage::Dissipating if self.params.v_max < 22.0 => {
                self.stage = TornadoStage::Dead;
            }
            _ => {}
        }
        let (v_goal, r_goal, tau_v, tau_r) = match self.stage {
            TornadoStage::Forming => (demand_v, self.target_r_max * 0.7, 25.0, 30.0),
            TornadoStage::Mature => {
                // stronger vortices broaden as they mature (wedge growth), if the cloud base is low
                let broaden = 1.0 + 0.6 * smoothstep(60.0, 95.0, demand_v) * smoothstep(1500.0, 500.0, self.lcl);
                (demand_v, self.target_r_max * broaden, 40.0, 70.0)
            }
            TornadoStage::Shrinking => (demand_v * 0.85, (self.target_r_max * 0.15).max(10.0), 90.0, 45.0),
            TornadoStage::Dissipating | TornadoStage::Dead => (0.0, 8.0, 25.0, 30.0),
        };
        let p = &mut self.params;
        p.v_max += (v_goal * (1.0 + gust) - p.v_max) * clamp01(dt / tau_v);
        p.r_max += (r_goal * (1.0 + width_noise) - p.r_max) * clamp01(dt / tau_r);
        p.r_max = p.r_max.clamp(4.0, 1500.0);
        p.v_max = p.v_max.clamp(0.0, 140.0);
        // two-celled structure grows with swirl: wide, intense vortices split their core
        let multi_on = self.overrides.multivortex.unwrap_or(self.multivortex.enabled || p.beta > 1.6);
        self.multivortex.enabled = multi_on;
        let beta_goal = if multi_on { 2.2 } else { 0.9 * smoothstep(120.0, 400.0, p.r_max) };
        p.beta += (beta_goal - p.beta) * clamp01(dt / 30.0);
        p.derive_layers();
        self.multivortex.step(&mut self.rng, dt, p.v_max, p.r_max, p.beta);

        // translation: the storm's motion, a drift towards the mesocyclone, a left turn in
        // the rope-out stage
        let mut v = match forcing {
            Some(f) => {
                let mut v = f.motion;
                let to_meso = (f.meso_center - self.pos).horizontal();
                v += to_meso * 0.01;
                v
            }
            None => self.velocity,
        };
        if matches!(self.stage, TornadoStage::Shrinking | TornadoStage::Dissipating) {
            // turn ~40 degrees to the left (cyclonically) and slow down
            let (s, c) = (0.7f64.sin(), 0.7f64.cos());
            let left = Vec3::new(c * v.x + s * v.z, 0.0, -s * v.x + c * v.z);
            v = v.lerp(left * 0.7, clamp01(dt / 20.0) * 20.0 / 20.0);
        }
        if let Some(speed) = self.overrides.speed {
            let dir = if v.horizontal_length() > 0.1 { v.horizontal().normalized() } else { Vec3::new(1.0, 0.0, 0.0) };
            v = dir * speed;
        }
        self.velocity = self.velocity.lerp(v.horizontal(), clamp01(dt / 8.0));
        self.pos += self.velocity * dt;

        // tilt: the cloud-base end lags behind as the vortex weakens and the RFD undercuts it
        let rope = matches!(self.stage, TornadoStage::Shrinking | TornadoStage::Dissipating);
        let tilt_goal = if rope {
            -self.velocity.normalized() * (0.55 * self.cloud_base) + self.velocity.normalized().cyclonic_tangent() * (0.3 * self.cloud_base)
        } else {
            -self.velocity.normalized() * (0.06 * self.cloud_base)
        };
        self.tilt = self.tilt.lerp(tilt_goal, clamp01(dt / 25.0));
        self.waviness = lerp(self.waviness, if rope { 0.75 } else { 0.12 }, clamp01(dt / 20.0));
        self.wave_phase += dt * (0.4 + 0.02 * self.params.v_max / self.params.r_max.max(1.0));
        // the rear flank sits behind and right of the motion
        if self.velocity.horizontal_length() > 0.5 {
            let (u, vn) = self.velocity.met();
            self.rfd_angle = vn.atan2(u) - 2.4;
        }
        // lofted debris settles out unless the damage engine keeps feeding it
        self.debris_load *= (-dt / 25.0).exp();
        self.update_funnel(1.15);
    }

    pub fn update_funnel(&mut self, rho: f64) {
        let inputs = FunnelInputs {
            lcl: self.lcl,
            cloud_base: self.cloud_base,
            tilt: self.tilt,
            waviness: self.waviness,
            wave_phase: self.wave_phase,
            rho,
            rain_wrap: self.rain_wrap,
        };
        self.funnel = compute_funnel(&self.params, &self.multivortex, &inputs);
    }

    pub fn morphology(&self) -> Morphology {
        self.funnel.morphology
    }

    /// Ground-relative wind (m/s) at world position `at`; `at.y` is its world height.
    pub fn wind_at(&self, at: Vec3) -> Vec3 {
        let rel = Vec3::new(at.x - self.pos.x, at.y - self.pos.y, at.z - self.pos.z);
        if rel.horizontal_length() > self.influence_radius() {
            return Vec3::ZERO;
        }
        let mut v = self.params.wind(rel);
        // wavenumber-1 asymmetry: the rear-flank side is stronger
        let theta = (-rel.z).atan2(rel.x);
        let k = 1.0 + self.asymmetry * (theta - self.rfd_angle).cos();
        v.x *= k;
        v.z *= k;
        v += self.multivortex.wind(rel);
        // translation, reduced slightly in the surface layer
        v += self.velocity * (0.85 + 0.15 * self.params.surface_factor(rel.y));
        v
    }

    /// Beyond this distance the vortex's own winds are negligible (< ~5 m/s).
    pub fn influence_radius(&self) -> f64 {
        (self.params.r_max * (self.params.v_max / 5.0).max(1.0)).min(self.params.r_max * 20.0).max(30.0)
    }

    /// Radius of winds at or above `threshold` m/s on the strong side, subvortices included.
    pub fn radius_of_wind(&self, threshold: f64) -> f64 {
        let trans = self.velocity.horizontal_length();
        let boosted = VortexParams { v_max: self.params.v_max * (1.0 + self.asymmetry), ..self.params };
        boosted.radius_of_wind(threshold, trans)
    }

    /// The strongest ground-relative wind the tornado can produce right now (m/s).
    pub fn peak_wind(&self) -> f64 {
        self.params.v_max * (1.0 + self.asymmetry) + self.velocity.horizontal_length() + self.multivortex.peak()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forcing() -> StormForcing {
        StormForcing {
            motion: Vec3::new(12.0, 0.0, -4.0),
            meso: 1.0,
            occluding: false,
            potential: 80.0,
            lcl: 600.0,
            cloud_base: 140.0,
            rain_wrap: 0.0,
            meso_center: Vec3::new(0.0, 64.0, 0.0),
        }
    }

    #[test]
    fn lifecycle_runs_to_dissipation() {
        let mut t = Tornado::new(1, Vec3::new(0.0, 64.0, 0.0), 1);
        t.life = 300.0;
        let mut stages = vec![t.stage];
        let mut peak = 0.0f64;
        let f = forcing();
        for _ in 0..(20 * 600) {
            t.step(0.05, Some(&f));
            peak = peak.max(t.params.v_max);
            if *stages.last().unwrap() != t.stage {
                stages.push(t.stage);
            }
            if !t.alive() {
                break;
            }
        }
        assert_eq!(
            stages,
            vec![
                TornadoStage::Forming,
                TornadoStage::Mature,
                TornadoStage::Shrinking,
                TornadoStage::Dissipating,
                TornadoStage::Dead
            ]
        );
        assert!(peak > 65.0, "{peak}");
    }

    #[test]
    fn right_side_is_stronger_than_left() {
        let mut t = Tornado::spawn(1, Vec3::new(0.0, 64.0, 0.0), SpawnShape::Cone, 5);
        t.velocity = Vec3::new(15.0, 0.0, 0.0); // moving east
        t.params = VortexParams::new(60.0, 40.0);
        // right of the motion (south, +z) the cyclonic flow is along +x like the motion
        let right = t.wind_at(Vec3::new(0.0, 74.0, 40.0)).horizontal_length();
        let left = t.wind_at(Vec3::new(0.0, 74.0, -40.0)).horizontal_length();
        assert!(right > left + 20.0, "{right} {left}");
    }

    #[test]
    fn rope_out_narrows_and_tilts() {
        let mut t = Tornado::spawn(1, Vec3::new(0.0, 64.0, 0.0), SpawnShape::Wedge, 9);
        let mut f = forcing();
        for _ in 0..(20 * 120) {
            t.step(0.05, Some(&f));
        }
        let wide = t.params.r_max;
        f.occluding = true;
        for _ in 0..(20 * 150) {
            t.step(0.05, Some(&f));
        }
        assert!(t.params.r_max < wide * 0.5, "{} vs {wide}", t.params.r_max);
        assert!(t.tilt.horizontal_length() > 0.3 * t.cloud_base);
    }

    #[test]
    fn command_overrides_hold() {
        let mut t = Tornado::spawn(1, Vec3::new(0.0, 64.0, 0.0), SpawnShape::Cone, 2);
        t.overrides.v_max = Some(100.0);
        t.overrides.speed = Some(5.0);
        for _ in 0..(20 * 200) {
            t.step(0.05, Some(&forcing()));
        }
        assert!((t.params.v_max - 100.0).abs() < 15.0, "{}", t.params.v_max);
        assert!((t.velocity.horizontal_length() - 5.0).abs() < 0.5);
    }
}
