// The weather of a world: the environment, its storms and their tornadoes, advanced on the
// server with a fixed step. Tornadogenesis is a chance that depends on the environment and the
// mesocyclone, so most supercells, even strongly rotating ones, produce no tornado.

use super::{Storm, StormKind, StormStage, SupercellType};
use crate::atmosphere::Environment;
use crate::atmosphere::presets::Preset;
use crate::config::WorldScale;
use crate::math::{Rng, Vec3, clamp01, smoothstep};
use crate::tornado::{SpawnShape, StormForcing, Tornado, TornadoStage};

/// Something that happened this step, for clients (sights, sounds) and the survey.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WeatherEvent {
    /// A cloud-to-ground strike at `pos` (world, ground level).
    Lightning { pos: Vec3, storm: u32 },
    TornadoFormed { id: u32, storm: Option<u32>, pos: Vec3 },
    TornadoDissipated { id: u32, pos: Vec3 },
    StormFormed { id: u32 },
}

/// Why a mesocyclone did or didn't produce a tornado (shown by `/storm debug`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GenesisOdds {
    /// Chance per storm-minute while the low-level mesocyclone is strong.
    pub per_minute: f64,
    /// The strongest peak wind the environment supports (m/s).
    pub potential: f64,
}

pub struct StormSystem {
    pub env: Environment,
    pub scale: WorldScale,
    pub storms: Vec<Storm>,
    pub tornadoes: Vec<Tornado>,
    pub events: Vec<WeatherEvent>,
    /// Reference ground height for storm-scale heights (world y).
    pub ground_y: f64,
    pub natural: bool,
    next_storm: u32,
    next_tornado: u32,
    initiation_timer: f64,
    rng: Rng,
    seed: u64,
}

impl StormSystem {
    pub fn new(env: Environment, scale: WorldScale, seed: u64) -> Self {
        StormSystem {
            env,
            scale,
            storms: Vec::new(),
            tornadoes: Vec::new(),
            events: Vec::new(),
            ground_y: 64.0,
            natural: false,
            next_storm: 1,
            next_tornado: 1,
            initiation_timer: 120.0,
            rng: Rng::new(seed),
            seed,
        }
    }

    pub fn set_environment(&mut self, env: Environment) {
        self.env = env;
        for s in &mut self.storms {
            s.lcl = self.env.sb.lcl.max(150.0);
            s.cloud_base = (s.lcl * self.scale.cloud_height).clamp(60.0, self.scale.max_cloud_base);
        }
    }

    pub fn set_preset(&mut self, p: Preset) {
        self.set_environment(Environment::from_preset(p));
    }

    /// The chance of tornadogenesis and the intensity the environment supports.
    pub fn genesis_odds(env: &Environment, meso_strength: f64, kind: StormKind) -> GenesisOdds {
        let stp = env.stp;
        let potential = 40.0 + 55.0 * clamp01((stp - 0.3) / 7.0).powf(0.6) + 10.0 * clamp01(env.srh_0_1 / 400.0);
        let tor = clamp01(stp / 4.0).powf(0.8) * smoothstep(0.5, 0.9, meso_strength);
        let kind_factor = match kind {
            StormKind::Supercell(SupercellType::LowPrecip) => 0.5,
            StormKind::Supercell(_) => 1.0,
            StormKind::Qlcs => 0.45,
            _ => 0.0,
        };
        // high cloud bases mean cold, negatively buoyant RFD air: few tornadoes
        let lcl = smoothstep(2000.0, 1000.0, env.sb.lcl);
        GenesisOdds { per_minute: 0.12 * tor * kind_factor * lcl, potential: potential.min(108.0) }
    }

    /// A new storm of `kind` that will pass near `target`: started upstream of it, at
    /// `distance` blocks (or right there with `distance` 0).
    pub fn spawn_storm(&mut self, kind: StormKind, target: Vec3, distance: f64) -> u32 {
        let id = self.next_storm;
        self.next_storm += 1;
        let probe = Storm::new(id, kind, target, &self.env, &self.scale, self.seed);
        let dir = if probe.motion.horizontal_length() > 0.5 { probe.motion.normalized() } else { Vec3::new(1.0, 0.0, 0.0) };
        // the updraft passes a little to the left so the hook and its tornado cross the target
        let lateral = dir.cyclonic_tangent() * (probe.updraft_radius * 0.3);
        let mut pos = target - dir * distance + lateral;
        pos.y = self.ground_y;
        let mut s = Storm::new(id, kind, pos, &self.env, &self.scale, self.seed.wrapping_add(id as u64 * 7919));
        if distance <= 0.0 {
            // a storm conjured in place starts mature
            s.age = 1500.0;
        }
        self.storms.push(s);
        self.events.push(WeatherEvent::StormFormed { id });
        id
    }

    /// A tornado of `shape` at `at`, under a supercell made for it if no storm is near.
    pub fn spawn_tornado(&mut self, shape: SpawnShape, at: Vec3) -> u32 {
        let storm_id = self
            .storms
            .iter()
            .filter(|s| s.kind.is_supercell() || s.kind == StormKind::Qlcs)
            .filter(|s| s.tornado.is_none())
            .min_by(|a, b| {
                (a.meso_center() - at).horizontal_length().total_cmp(&(b.meso_center() - at).horizontal_length())
            })
            .filter(|s| (s.meso_center() - at).horizontal_length() < 1500.0)
            .map(|s| s.id);
        let storm_id = match storm_id {
            Some(id) => id,
            None => {
                let kind = if shape == SpawnShape::RainWrapped {
                    StormKind::Supercell(SupercellType::HighPrecip)
                } else {
                    StormKind::Supercell(SupercellType::Classic)
                };
                let id = self.spawn_storm(kind, at, 0.0);
                let s = self.storms.iter_mut().find(|s| s.id == id).unwrap();
                s.meso.strength = 0.85;
                s.meso.v_rot = 12.0 + 28.0 * 0.85;
                // put the mesocyclone over the tornado
                s.pos = at - s.meso.offset;
                s.pos.y = self.ground_y;
                id
            }
        };
        let id = self.next_tornado;
        self.next_tornado += 1;
        let mut t = Tornado::spawn(id, at, shape, self.seed.wrapping_add(id as u64 * 104_729));
        t.storm_id = Some(storm_id);
        if let Some(s) = self.storms.iter_mut().find(|s| s.id == storm_id) {
            s.tornado = Some(id);
            s.tornadoes_produced += 1;
            if shape == SpawnShape::RainWrapped {
                s.rain_wrap = 0.9;
            }
            t.cloud_base = s.cloud_base;
            t.velocity = s.motion;
        }
        if shape == SpawnShape::Satellite {
            // a satellite needs a primary to orbit: the strongest tornado near
            t.satellite_of = self
                .tornadoes
                .iter()
                .filter(|o| (o.pos - at).horizontal_length() < 2000.0)
                .max_by(|a, b| a.params.v_max.total_cmp(&b.params.v_max))
                .map(|o| o.id);
        }
        self.events.push(WeatherEvent::TornadoFormed { id, storm: Some(storm_id), pos: at });
        self.tornadoes.push(t);
        id
    }

    pub fn tornado_mut(&mut self, id: u32) -> Option<&mut Tornado> {
        self.tornadoes.iter_mut().find(|t| t.id == id)
    }

    /// The live tornado nearest to `at`.
    pub fn nearest_tornado(&mut self, at: Vec3) -> Option<&mut Tornado> {
        self.tornadoes
            .iter_mut()
            .filter(|t| t.alive())
            .min_by(|a, b| (a.pos - at).horizontal_length().total_cmp(&(b.pos - at).horizontal_length()))
    }

    /// Advance everything by `dt` real seconds.
    pub fn step(&mut self, dt: f64) {
        let storm_dt = dt * self.scale.storm_time;
        let env = &self.env;
        for s in &mut self.storms {
            s.step(storm_dt, env, dt);
        }
        // tornadogenesis
        let mut births = Vec::new();
        for s in &mut self.storms {
            if s.tornado.is_some() || !matches!(s.stage, StormStage::Mature | StormStage::Intensifying) && !s.force_tornado {
                continue;
            }
            let odds = Self::genesis_odds(&self.env, s.meso.strength, s.kind);
            let ready = s.genesis_cooldown <= 0.0 && s.meso.occlusion < 0.5;
            let p = odds.per_minute * storm_dt / 60.0;
            if s.force_tornado || (ready && s.rng.chance(p)) {
                s.force_tornado = false;
                births.push((s.id, s.meso_center(), odds.potential, s.kind));
            } else if ready && s.meso.strength > 0.6 && odds.per_minute < 0.01 {
                // a strongly rotating storm in a hostile environment: try again later
                s.genesis_cooldown = 600.0;
            }
        }
        for (storm_id, at, potential, kind) in births {
            self.birth(storm_id, at, potential, kind);
        }
        // tornadoes follow their storms
        let mut satellites = Vec::new();
        for i in 0..self.tornadoes.len() {
            let storm_id = self.tornadoes[i].storm_id;
            let forcing = storm_id.and_then(|id| self.storms.iter().find(|s| s.id == id)).map(|s| StormForcing {
                motion: s.motion,
                meso: s.meso.strength,
                occluding: s.meso.occlusion > 0.8 || matches!(s.stage, StormStage::Dissipating | StormStage::Dead),
                potential: self.tornadoes[i].target_v_max.max(30.0),
                lcl: s.lcl,
                cloud_base: s.cloud_base,
                rain_wrap: s.rain_wrap,
                meso_center: s.meso_center(),
            });
            // a satellite circles its primary
            if let Some(primary) = self.tornadoes[i].satellite_of {
                if let Some(p) = self.tornadoes.iter().find(|t| t.id == primary && t.alive()).map(|t| (t.pos, t.velocity, t.params.r_max)) {
                    let rel = (self.tornadoes[i].pos - p.0).horizontal();
                    let dist = rel.length().max(1.0);
                    let orbit = (p.2 * 3.0).max(150.0);
                    let tangential = rel.normalized().cyclonic_tangent() * 18.0;
                    let radial = -rel.normalized() * ((dist - orbit) * 0.05);
                    self.tornadoes[i].overrides.speed = None;
                    self.tornadoes[i].velocity = p.1 + tangential + radial;
                    satellites.push(i);
                }
            }
            let f = if satellites.last() == Some(&i) { None } else { forcing };
            self.tornadoes[i].step(dt, f.as_ref());
            self.tornadoes[i].pos.y = self.tornadoes[i].pos.y.max(-64.0);
        }
        // a large, violent tornado may spawn a satellite
        let mut spawn_sat = Vec::new();
        for t in &self.tornadoes {
            if t.stage == TornadoStage::Mature && t.params.v_max > 75.0 && t.params.r_max > 120.0 && t.satellite_of.is_none() {
                let has = self.tornadoes.iter().any(|o| o.satellite_of == Some(t.id) && o.alive());
                if !has && self.rng.chance(dt / 600.0) {
                    let ang = self.rng.range(0.0, std::f64::consts::TAU);
                    let d = (t.params.r_max * 3.0).max(150.0);
                    spawn_sat.push(t.pos + Vec3::new(ang.cos() * d, 0.0, ang.sin() * d));
                }
            }
        }
        for at in spawn_sat {
            self.spawn_tornado(SpawnShape::Satellite, at);
        }
        // the dead
        for t in &self.tornadoes {
            if !t.alive() {
                self.events.push(WeatherEvent::TornadoDissipated { id: t.id, pos: t.pos });
                if let Some(s) = t.storm_id.and_then(|id| self.storms.iter_mut().find(|s| s.id == id)) {
                    if s.tornado == Some(t.id) {
                        s.tornado = None;
                        s.genesis_cooldown = 900.0;
                    }
                }
            }
        }
        self.tornadoes.retain(|t| t.alive());
        self.storms.retain(|s| s.alive());
        self.lightning(dt);
        if self.natural {
            self.initiate(dt);
        }
    }

    fn birth(&mut self, storm_id: u32, at: Vec3, potential: f64, kind: StormKind) {
        let rng = &mut self.rng;
        // most tornadoes are far weaker than what the environment allows
        let draw = 0.5 + 0.5 * rng.f64().powf(1.8);
        let v = (potential * draw).clamp(32.0, 115.0);
        let size = 25.0 + 260.0 * rng.f64().powi(2) * (v / 85.0).powf(1.5);
        let shape = if kind == StormKind::Supercell(SupercellType::HighPrecip) {
            SpawnShape::RainWrapped
        } else if v > 70.0 && size > 160.0 && self.env.sb.lcl > 1100.0 {
            SpawnShape::MultipleVortex
        } else if size > 200.0 {
            SpawnShape::Wedge
        } else if size > 70.0 {
            SpawnShape::Stovepipe
        } else {
            SpawnShape::Cone
        };
        let id = self.spawn_tornado(shape, Vec3::new(at.x, self.ground_y, at.z));
        if let Some(t) = self.tornadoes.iter_mut().find(|t| t.id == id) {
            t.storm_id = Some(storm_id);
            t.target_v_max = v;
            t.target_r_max = size;
            t.lcl = self.env.sb.lcl;
            t.life = (120.0 + 1300.0 * clamp01((v - 32.0) / 70.0).powf(1.3)) * self.rng.range(0.6, 1.4);
            t.multivortex.enabled = shape == SpawnShape::MultipleVortex;
        }
    }

    fn lightning(&mut self, dt: f64) {
        for s in &mut self.storms {
            // CG flash rate rises steeply with updraft strength; a few per minute in a strong storm
            let per_minute = (0.004 * s.updraft * s.updraft).min(12.0) * s.precip;
            if s.rng.chance(per_minute * dt / 60.0) {
                let ru = s.updraft_radius;
                let at = s.pos + s.downshear * (ru * s.rng.range(0.2, 2.2)) + s.downshear.cyclonic_tangent() * (ru * s.rng.range(-1.0, 1.0));
                self.events.push(WeatherEvent::Lightning { pos: Vec3::new(at.x, self.ground_y, at.z), storm: s.id });
            }
        }
    }

    fn initiate(&mut self, dt: f64) {
        self.initiation_timer -= dt * self.scale.storm_time;
        if self.initiation_timer > 0.0 || self.storms.len() >= 4 {
            return;
        }
        self.initiation_timer = self.rng.range(600.0, 1800.0);
        let cape = self.env.sb.cape.max(self.env.ml.cape);
        let p = smoothstep(300.0, 2000.0, cape) * smoothstep(-150.0, -20.0, self.env.sb.cin);
        if self.rng.chance(p) {
            let kind = StormKind::favoured_by(&self.env);
            let ang = self.rng.range(0.0, std::f64::consts::TAU);
            let d = self.rng.range(0.5, 1.5) * 8000.0 * self.scale.storm_length * 4.0;
            let at = Vec3::new(ang.cos() * d, self.ground_y, ang.sin() * d);
            self.spawn_storm(kind, at, 0.0);
        }
    }

    /// Composite wind (m/s) at world `at` (`at.y` world height): environment, every storm's
    /// own winds and every tornado. Used for straight-line damage, particles and debris.
    pub fn wind_at(&self, at: Vec3) -> Vec3 {
        let h = (at.y - self.ground_y).max(0.5);
        let ambient = match self.storm_near(at) {
            Some(s) => s.wind(&self.env, at, h),
            None => self.env.wind(h),
        };
        // a tornado's ground-relative field (its own flow plus its translation) replaces the
        // ambient flow near its core instead of adding to it, fading out over its outer flow
        let mut own = Vec3::ZERO;
        let mut weight: f64 = 0.0;
        for t in &self.tornadoes {
            let d = (at - t.pos).horizontal_length();
            let inner = 3.0 * t.params.r_max;
            let outer = t.influence_radius().max(inner * 1.5);
            if d < outer {
                own += t.wind_at(at);
                weight = weight.max(1.0 - smoothstep(inner, outer, d));
            }
        }
        ambient * (1.0 - weight) + own
    }

    /// The storm whose winds dominate at `at` (the nearest updraft within its extent).
    pub fn storm_near(&self, at: Vec3) -> Option<&Storm> {
        self.storms
            .iter()
            .filter(|s| (s.pos - at).horizontal_length() < s.extent() * 1.2)
            .min_by(|a, b| (a.pos - at).horizontal_length().total_cmp(&(b.pos - at).horizontal_length()))
    }

    pub fn clear(&mut self) {
        for t in &self.tornadoes {
            self.events.push(WeatherEvent::TornadoDissipated { id: t.id, pos: t.pos });
        }
        self.storms.clear();
        self.tornadoes.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genesis_is_rare_in_marginal_air_and_likelier_in_an_outbreak() {
        let k = StormKind::Supercell(SupercellType::Classic);
        let out = StormSystem::genesis_odds(&Environment::from_preset(Preset::Outbreak), 0.9, k);
        let marg = StormSystem::genesis_odds(&Environment::from_preset(Preset::Marginal), 0.9, k);
        let lp = StormSystem::genesis_odds(&Environment::from_preset(Preset::LowPrecip), 0.9, k);
        assert!(out.per_minute > 0.05, "{out:?}");
        assert!(marg.per_minute < 0.01, "{marg:?}");
        assert!(lp.per_minute < out.per_minute * 0.3, "{lp:?}");
        assert!(out.potential > 75.0 && out.potential <= 108.0);
        // no mesocyclone, no tornado
        assert_eq!(StormSystem::genesis_odds(&Environment::from_preset(Preset::Outbreak), 0.2, k).per_minute, 0.0);
    }

    #[test]
    fn not_every_supercell_is_tornadic() {
        // many storms in a classic environment: some tornadic, some not
        let mut tornadic = 0;
        let n = 24;
        for seed in 0..n {
            let mut sys = StormSystem::new(Environment::from_preset(Preset::Classic), WorldScale::default(), seed);
            sys.spawn_storm(StormKind::Supercell(SupercellType::Classic), Vec3::ZERO, 0.0);
            let mut had = false;
            for _ in 0..(20 * 60 * 12) {
                sys.step(0.05);
                if !sys.tornadoes.is_empty() {
                    had = true;
                    break;
                }
                if sys.storms.is_empty() {
                    break;
                }
            }
            tornadic += had as u32;
        }
        assert!(tornadic > 0 && tornadic < n as u32, "{tornadic} of {n}");
    }

    #[test]
    fn spawned_tornado_has_a_parent_storm_and_moves_with_it() {
        let mut sys = StormSystem::new(Environment::from_preset(Preset::Outbreak), WorldScale::default(), 1);
        let id = sys.spawn_tornado(SpawnShape::Wedge, Vec3::new(100.0, 64.0, 100.0));
        let storm = sys.tornadoes[0].storm_id.unwrap();
        assert!(sys.storms.iter().any(|s| s.id == storm && s.tornado == Some(id)));
        let start = sys.tornadoes[0].pos;
        for _ in 0..(20 * 60) {
            sys.step(0.05);
        }
        let t = sys.tornadoes.iter().find(|t| t.id == id).unwrap();
        assert!((t.pos - start).horizontal_length() > 200.0);
        // its wind is part of the composite field
        let near = t.pos + Vec3::new(t.params.r_max, 10.0, 0.0);
        assert!(sys.wind_at(near).horizontal_length() > 40.0);
    }

    #[test]
    fn lightning_happens_in_strong_storms() {
        let mut sys = StormSystem::new(Environment::from_preset(Preset::Outbreak), WorldScale::default(), 5);
        sys.spawn_storm(StormKind::Supercell(SupercellType::Classic), Vec3::ZERO, 0.0);
        let mut strikes = 0;
        for _ in 0..(20 * 300) {
            sys.step(0.05);
            strikes += sys.events.iter().filter(|e| matches!(e, WeatherEvent::Lightning { .. })).count();
            sys.events.clear();
        }
        assert!(strikes > 3, "{strikes}");
    }
}
