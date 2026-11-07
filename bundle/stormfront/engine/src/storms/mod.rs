// Convective storms: their kinds, life cycle, mesocyclone and hydrometeors.
//
// A storm is a moving updraft with precipitation regions placed around it according to its
// kind and the environment's winds (downshear forward-flank core, rear-flank hook, BWER under
// the echo overhang). Storm-scale lengths are compressed by `WorldScale::storm_length` and its
// evolution sped up by `WorldScale::storm_time` (see config.rs); tornadoes it spawns run at
// full scale and in real time.

pub mod fields;
pub mod system;

use crate::atmosphere::Environment;
use crate::math::{Rng, Vec3, clamp01, smoothstep};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupercellType {
    Classic,
    HighPrecip,
    LowPrecip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StormKind {
    /// An ordinary single cell (pulse storm).
    Single,
    Multicell,
    Supercell(SupercellType),
    /// A quasi-linear convective system (squall line / bow echo) with embedded mesovortices.
    Qlcs,
}

impl StormKind {
    pub fn name(self) -> &'static str {
        match self {
            StormKind::Single => "single cell",
            StormKind::Multicell => "multicell",
            StormKind::Supercell(SupercellType::Classic) => "classic supercell",
            StormKind::Supercell(SupercellType::HighPrecip) => "HP supercell",
            StormKind::Supercell(SupercellType::LowPrecip) => "LP supercell",
            StormKind::Qlcs => "QLCS",
        }
    }
    pub fn id(self) -> u8 {
        match self {
            StormKind::Single => 0,
            StormKind::Multicell => 1,
            StormKind::Supercell(SupercellType::Classic) => 2,
            StormKind::Supercell(SupercellType::HighPrecip) => 3,
            StormKind::Supercell(SupercellType::LowPrecip) => 4,
            StormKind::Qlcs => 5,
        }
    }
    pub fn from_id(id: u8) -> Self {
        match id {
            1 => StormKind::Multicell,
            2 => StormKind::Supercell(SupercellType::Classic),
            3 => StormKind::Supercell(SupercellType::HighPrecip),
            4 => StormKind::Supercell(SupercellType::LowPrecip),
            5 => StormKind::Qlcs,
            _ => StormKind::Single,
        }
    }
    pub fn is_supercell(self) -> bool {
        matches!(self, StormKind::Supercell(_))
    }
    /// Parse a `/storm <kind>` argument.
    pub fn parse(s: &str) -> Option<StormKind> {
        Some(match s.to_ascii_lowercase().as_str() {
            "single" | "cell" | "pulse" | "comum" => StormKind::Single,
            "multicell" | "multi" | "multicelula" | "multicélula" => StormKind::Multicell,
            "supercell" | "classic" | "supercelula" | "supercélula" => StormKind::Supercell(SupercellType::Classic),
            "hp" => StormKind::Supercell(SupercellType::HighPrecip),
            "lp" => StormKind::Supercell(SupercellType::LowPrecip),
            "qlcs" | "line" | "squall" | "linha" => StormKind::Qlcs,
            _ => return None,
        })
    }
    pub const NAMES: &'static str = "supercell|hp|lp|multicell|single|qlcs|microburst";

    /// The kind the environment favours (storm mode from shear, helicity and moisture).
    pub fn favoured_by(env: &Environment) -> StormKind {
        if env.shear_0_6 >= 18.0 && env.srh_0_3 >= 100.0 && env.sb.cape.max(env.ml.cape) >= 500.0 {
            if env.shear_0_6 >= 25.0 && env.sb.cape < 1200.0 && env.mean_wind_speed() > 18.0 {
                return StormKind::Qlcs;
            }
            let pe = env.precip_efficiency();
            let t = if pe > 0.72 || env.anvil_sr_wind < 12.0 {
                SupercellType::HighPrecip
            } else if pe < 0.35 || env.anvil_sr_wind > 30.0 {
                SupercellType::LowPrecip
            } else {
                SupercellType::Classic
            };
            StormKind::Supercell(t)
        } else if env.shear_0_6 >= 10.0 {
            StormKind::Multicell
        } else {
            StormKind::Single
        }
    }
}

impl Environment {
    pub fn mean_wind_speed(&self) -> f64 {
        (self.mean_wind.0 * self.mean_wind.0 + self.mean_wind.1 * self.mean_wind.1).sqrt()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StormStage {
    #[default]
    Developing,
    Mature,
    /// Low-level rotation strengthening (supercells).
    Intensifying,
    /// A tornado is on the ground.
    Tornadic,
    /// The mesocyclone occludes under the wrapping RFD; a new one may form downshear.
    Occluding,
    Dissipating,
    Dead,
}

impl StormStage {
    pub fn name(self) -> &'static str {
        match self {
            StormStage::Developing => "developing",
            StormStage::Mature => "mature",
            StormStage::Intensifying => "intensifying",
            StormStage::Tornadic => "tornadic",
            StormStage::Occluding => "occluding",
            StormStage::Dissipating => "dissipating",
            StormStage::Dead => "dead",
        }
    }
    pub fn id(self) -> u8 {
        self as u8
    }
    pub fn from_id(id: u8) -> Self {
        match id {
            1 => StormStage::Mature,
            2 => StormStage::Intensifying,
            3 => StormStage::Tornadic,
            4 => StormStage::Occluding,
            5 => StormStage::Dissipating,
            6 => StormStage::Dead,
            _ => StormStage::Developing,
        }
    }
}

/// The rotating updraft of a supercell (or a QLCS mesovortex).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mesocyclone {
    /// Low-level rotation strength (0..1).
    pub strength: f64,
    /// Radius of peak rotational velocity (world blocks).
    pub radius: f64,
    /// Peak rotational velocity (m/s).
    pub v_rot: f64,
    /// Offset of its centre from the updraft centre (world blocks).
    pub offset: Vec3,
    /// How far the RFD gust front has wrapped around it (0..1).
    pub occlusion: f64,
    /// Cycles completed (cyclic mesocyclogenesis).
    pub cycle: u32,
}

/// A downburst at the ground.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Microburst {
    pub pos: Vec3,
    pub age: f64,
    pub life: f64,
    /// Peak outflow (m/s).
    pub strength: f64,
    /// Radius of peak outflow (blocks).
    pub radius: f64,
}

impl Microburst {
    pub fn envelope(&self) -> f64 {
        let t = (self.age / self.life).clamp(0.0, 1.0);
        smoothstep(0.0, 0.2, t) * (1.0 - smoothstep(0.55, 1.0, t))
    }
}

#[derive(Clone, Debug)]
pub struct Storm {
    pub id: u32,
    pub kind: StormKind,
    pub stage: StormStage,
    /// Updraft centre at the ground (world coordinates; y = reference ground height).
    pub pos: Vec3,
    pub motion: Vec3,
    /// Storm-clock age and expected life (s of storm time).
    pub age: f64,
    pub life: f64,
    /// Peak updraft (m/s) and its radius (blocks).
    pub updraft: f64,
    pub updraft_radius: f64,
    /// Rain production (0..1) and hail (0..1) intensities.
    pub precip: f64,
    pub hail: f64,
    /// Maximum hail diameter (cm).
    pub hail_size: f64,
    pub meso: Mesocyclone,
    /// Rear-flank and forward-flank downdraft strengths (0..1).
    pub rfd: f64,
    pub ffd: f64,
    /// Direction precipitation is carried (unit, horizontal): the anvil-level storm-relative wind.
    pub downshear: Vec3,
    /// Physical LCL (m) and display cloud base (blocks above ground).
    pub lcl: f64,
    pub cloud_base: f64,
    /// Cloud top (m, physical).
    pub top: f64,
    /// Fraction of the mesocyclone wrapped in rain (HP storms).
    pub rain_wrap: f64,
    /// QLCS: half-length of the line (blocks) and its orientation (unit along the line).
    pub line_half_length: f64,
    pub line_dir: Vec3,
    /// Multicell: offsets of the daughter cells and their ages (storm s).
    pub cells: Vec<(Vec3, f64)>,
    pub microbursts: Vec<Microburst>,
    pub tornado: Option<u32>,
    pub tornadoes_produced: u32,
    /// Storm-time seconds until tornadogenesis may be tried again.
    pub genesis_cooldown: f64,
    /// A command asked for a tornado as soon as possible.
    pub force_tornado: bool,
    pub rng: Rng,
}

impl Storm {
    pub fn new(id: u32, kind: StormKind, pos: Vec3, env: &Environment, scale: &crate::config::WorldScale, seed: u64) -> Self {
        let mut rng = Rng::new(seed ^ (id as u64) << 20);
        let anvil = env.sounding.mean_wind(9000.0, 11000.0);
        let rm = env.right_mover;
        let motion = match kind {
            StormKind::Supercell(_) => env.storm_motion(),
            // ordinary cells move with the mean wind; lines with their propagation added
            StormKind::Qlcs => Vec3::from_met(env.mean_wind.0, env.mean_wind.1, 0.0) * 1.2,
            _ => Vec3::from_met(env.mean_wind.0, env.mean_wind.1, 0.0),
        };
        let mut downshear = Vec3::from_met(anvil.0 - rm.0, anvil.1 - rm.1, 0.0).normalized();
        if downshear.length_sq() < 0.5 {
            downshear = Vec3::new(1.0, 0.0, 0.0);
        }
        let s = scale.storm_length;
        let cape = env.sb.cape.max(env.ml.cape);
        let (life_min, updraft_r) = match kind {
            StormKind::Single => (40.0, 2500.0),
            StormKind::Multicell => (120.0, 3000.0),
            StormKind::Supercell(_) => (180.0, 4500.0),
            StormKind::Qlcs => (300.0, 3500.0),
        };
        let lcl = env.sb.lcl.max(150.0);
        let line_dir = motion.normalized().cyclonic_tangent();
        Storm {
            id,
            kind,
            stage: StormStage::Developing,
            pos,
            motion,
            age: 0.0,
            life: life_min * 60.0 * rng.range(0.8, 1.3),
            updraft: 0.0,
            updraft_radius: updraft_r * s,
            precip: 0.0,
            hail: 0.0,
            hail_size: 0.0,
            meso: Mesocyclone { radius: 3000.0 * s, ..Mesocyclone::default() },
            rfd: 0.0,
            ffd: 0.0,
            downshear,
            lcl,
            cloud_base: (lcl * scale.cloud_height).clamp(60.0, scale.max_cloud_base),
            top: env.sb.el.unwrap_or(10_000.0).max(6000.0),
            rain_wrap: 0.0,
            line_half_length: if kind == StormKind::Qlcs { 60_000.0 * s } else { 0.0 },
            line_dir: if line_dir.length_sq() > 0.5 { line_dir } else { Vec3::new(0.0, 0.0, 1.0) },
            cells: Vec::new(),
            microbursts: Vec::new(),
            tornado: None,
            tornadoes_produced: 0,
            genesis_cooldown: 600.0,
            force_tornado: false,
            rng,
        }
        .with_cape(cape)
    }

    fn with_cape(mut self, cape: f64) -> Self {
        // hail size grows with updraft speed (CAPE) and with shear organising the updraft
        self.hail_size = (0.6 * (2.0 * cape).sqrt() / 10.0).clamp(0.0, 12.0);
        self
    }

    pub fn alive(&self) -> bool {
        self.stage != StormStage::Dead
    }

    /// Mesocyclone centre in the world.
    pub fn meso_center(&self) -> Vec3 {
        self.pos + self.meso.offset
    }

    /// Advance by `dt` storm-time seconds (the caller multiplies by `storm_time`).
    pub fn step(&mut self, dt: f64, env: &Environment, real_dt: f64) {
        self.age += dt;
        self.pos += self.motion * real_dt;
        let life = self.age / self.life.max(1.0);
        let cape = env.sb.cape.max(env.ml.cape);
        let w_pot = (2.0 * cape).sqrt() * 0.55; // entrainment and loading roughly halve parcel theory
        // updraft rises over ~10 min, holds, decays
        let grow = smoothstep(0.0, 600.0, self.age);
        let decay = 1.0 - smoothstep(0.85, 1.0, life);
        let organised = if self.kind.is_supercell() { 1.0 } else { 0.75 };
        self.updraft = w_pot * grow * decay * organised;
        // precipitation starts ~12 min after initiation and peaks with the mature stage
        let pe = env.precip_efficiency();
        let type_precip = match self.kind {
            StormKind::Supercell(SupercellType::HighPrecip) => 1.0,
            StormKind::Supercell(SupercellType::LowPrecip) => 0.35,
            StormKind::Qlcs => 0.9,
            _ => 0.8,
        };
        self.precip = smoothstep(500.0, 1500.0, self.age) * decay.max(0.25 * (1.0 - smoothstep(1.0, 1.15, life))) * type_precip * (0.5 + 0.5 * pe);
        self.hail = self.precip * smoothstep(15.0, 40.0, self.updraft) * (1.0 - 0.5 * pe);
        self.ffd = self.precip * 0.8;
        // stages
        self.stage = if life >= 1.15 {
            StormStage::Dead
        } else if life >= 0.85 {
            StormStage::Dissipating
        } else if self.tornado.is_some() {
            if self.meso.occlusion > 0.75 { StormStage::Occluding } else { StormStage::Tornadic }
        } else if self.meso.occlusion > 0.75 {
            StormStage::Occluding
        } else if self.age < 900.0 {
            StormStage::Developing
        } else if self.kind.is_supercell() && self.meso.strength > 0.6 {
            StormStage::Intensifying
        } else {
            StormStage::Mature
        };
        match self.kind {
            StormKind::Supercell(t) => self.step_mesocyclone(dt, env, t),
            StormKind::Qlcs => self.step_line(dt, env),
            StormKind::Multicell => self.step_multicell(dt),
            StormKind::Single => {}
        }
        self.step_microbursts(dt, real_dt, env);
        self.genesis_cooldown -= dt;
    }

    fn step_mesocyclone(&mut self, dt: f64, env: &Environment, t: SupercellType) {
        // mid-level rotation from the tilting of streamwise vorticity: helicity and shear;
        // low-level rotation also needs low-level helicity and buoyancy near the ground
        let mid = clamp01(env.srh_0_3 / 250.0) * clamp01(env.shear_0_6 / 22.0);
        let low = clamp01(env.srh_0_1 / 200.0) * clamp01((env.sb.cape_0_3km + 40.0) / 120.0) * smoothstep(2200.0, 900.0, env.sb.lcl);
        let goal = if matches!(self.stage, StormStage::Dissipating | StormStage::Dead) {
            0.0
        } else {
            (0.45 * mid + 0.55 * low).max(0.4 * mid) * smoothstep(600.0, 1500.0, self.age)
        };
        let rate = if goal > self.meso.strength { dt / 400.0 } else { dt / 250.0 };
        self.meso.strength += (goal - self.meso.strength) * clamp01(rate);
        self.meso.v_rot = 12.0 + 28.0 * self.meso.strength;
        // the RFD strengthens with the low-level mesocyclone and wraps around it
        self.rfd = self.precip * (0.3 + 0.7 * self.meso.strength);
        let wrap_rate = if self.tornado.is_some() { dt / 900.0 } else { dt / 2400.0 } * self.rfd;
        self.meso.occlusion = (self.meso.occlusion + wrap_rate).min(1.0);
        // the meso sits on the updraft's rear flank and drifts rearward-left as it occludes
        let back = -self.motion.normalized();
        let left = self.motion.normalized().cyclonic_tangent();
        self.meso.offset = back * (self.updraft_radius * (0.25 + 0.6 * self.meso.occlusion))
            + left * (self.updraft_radius * 0.5 * self.meso.occlusion);
        if self.meso.occlusion >= 1.0 && self.tornado.is_none() {
            // cyclic mesocyclogenesis: a new low-level meso forms downshear
            self.meso.occlusion = 0.0;
            self.meso.strength *= 0.45;
            self.meso.cycle += 1;
            self.genesis_cooldown = self.genesis_cooldown.max(900.0);
        }
        self.rain_wrap = match t {
            SupercellType::HighPrecip => 0.55 + 0.4 * self.meso.strength,
            SupercellType::Classic => 0.15 + 0.25 * self.meso.occlusion,
            SupercellType::LowPrecip => 0.02,
        };
    }

    fn step_line(&mut self, dt: f64, env: &Environment) {
        // embedded mesovortices along the leading edge: weaker, shallower, short-lived
        let goal = clamp01(env.srh_0_1 / 250.0) * clamp01(env.shear_0_3 / 20.0) * 0.8;
        self.meso.strength += (goal - self.meso.strength) * clamp01(dt / 600.0);
        self.meso.v_rot = 10.0 + 20.0 * self.meso.strength;
        self.meso.radius = self.updraft_radius * 0.4;
        let along = self.line_dir * (self.line_half_length * (self.rng.range(-0.3, 0.3)));
        self.meso.offset = self.meso.offset.lerp(along, clamp01(dt / 1200.0));
        self.rfd = self.precip;
        self.rain_wrap = 0.6;
    }

    fn step_multicell(&mut self, dt: f64) {
        for c in &mut self.cells {
            c.1 += dt;
        }
        self.cells.retain(|c| c.1 < 2400.0);
        // new cells grow on the right / forward flank every ~10 minutes
        if self.cells.len() < 5 && self.rng.chance(dt / 600.0) && self.stage != StormStage::Dissipating {
            let right = -self.motion.normalized().cyclonic_tangent();
            let fwd = self.motion.normalized();
            let off = right * (self.updraft_radius * self.rng.range(0.8, 2.0)) + fwd * (self.updraft_radius * self.rng.range(-0.5, 1.0));
            self.cells.push((off, 0.0));
        }
    }

    fn step_microbursts(&mut self, dt: f64, real_dt: f64, env: &Environment) {
        for m in &mut self.microbursts {
            m.age += real_dt;
        }
        self.microbursts.retain(|m| m.age < m.life);
        // collapsing cores and heavy precipitation loading spawn downbursts
        let collapsing = matches!(self.stage, StormStage::Dissipating) || self.kind == StormKind::Single && self.age > 1500.0;
        let index = env.microburst_index();
        let rate = (if collapsing { 1.0 } else { 0.25 }) * index * self.precip / 900.0;
        if self.microbursts.len() < 2 && self.rng.chance(rate * dt) {
            let fwd = self.motion.normalized();
            let at = self.pos + fwd * (self.updraft_radius * self.rng.range(0.3, 1.2)) + self.downshear * (self.updraft_radius * self.rng.range(0.0, 0.8));
            self.microbursts.push(Microburst {
                pos: at,
                age: 0.0,
                life: self.rng.range(240.0, 600.0),
                // sqrt(2 DCAPE) is the theoretical limit; real peaks are about a third
                strength: ((2.0 * env.dcape).sqrt() * self.rng.range(0.28, 0.5)).clamp(15.0, 65.0),
                radius: self.rng.range(400.0, 1200.0) * 0.5,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atmosphere::presets::Preset;
    use crate::config::WorldScale;

    #[test]
    fn storm_mode_follows_the_environment() {
        let k = |p| StormKind::favoured_by(&Environment::from_preset(p));
        assert!(k(Preset::Outbreak).is_supercell());
        assert_eq!(k(Preset::HighPrecip), StormKind::Supercell(SupercellType::HighPrecip));
        assert_eq!(k(Preset::LowPrecip), StormKind::Supercell(SupercellType::LowPrecip));
        assert_eq!(k(Preset::Pulse), StormKind::Single);
        assert_eq!(k(Preset::Qlcs), StormKind::Qlcs);
    }

    #[test]
    fn supercell_life_cycle_builds_a_mesocyclone_then_dies() {
        let env = Environment::from_preset(Preset::Outbreak);
        let mut s = Storm::new(1, StormKind::Supercell(SupercellType::Classic), Vec3::ZERO, &env, &WorldScale::default(), 1);
        let mut max_meso = 0.0f64;
        let mut stages = Vec::new();
        for _ in 0..20_000 {
            s.step(1.0, &env, 0.25);
            max_meso = max_meso.max(s.meso.strength);
            if stages.last() != Some(&s.stage) {
                stages.push(s.stage);
            }
            if !s.alive() {
                break;
            }
        }
        assert!(max_meso > 0.6, "{max_meso}");
        assert!(stages.contains(&StormStage::Mature) || stages.contains(&StormStage::Intensifying));
        assert_eq!(*stages.last().unwrap(), StormStage::Dead);
        assert!(s.pos.horizontal_length() > 1000.0, "the storm moves");
    }

    #[test]
    fn weak_shear_supercell_has_weak_rotation() {
        let env = Environment::from_preset(Preset::Pulse);
        let mut s = Storm::new(1, StormKind::Supercell(SupercellType::Classic), Vec3::ZERO, &env, &WorldScale::default(), 1);
        for _ in 0..3000 {
            s.step(1.0, &env, 0.25);
        }
        assert!(s.meso.strength < 0.2, "{}", s.meso.strength);
    }
}
