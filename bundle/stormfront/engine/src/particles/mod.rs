// The client's particles: tornado condensation, dust and debris clouds, cloud bases and wall
// clouds, rain, hail, rain shafts, scud, lightning and block bursts.
//
// Particles follow the same wind field the server's damage uses (rebuilt from its messages),
// relaxing towards it with a response time that depends on what they are (cloud droplets follow
// the air almost at once; dirt and splinters lag, fall and are flung outward), so the funnel's
// helical motion, the inflow and the debris cloud come from the flow itself. They are updated at
// the game's 20 Hz and drawn with their velocity extrapolated each frame. The quality setting
// changes how many there are, never what the server does.

pub mod emitters;

use crate::config::Quality;
use crate::math::{Rng, Vec3};
use crate::render_bridge::{Instance, Shape};
use crate::storms::system::StormSystem;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    pub pos: Vec3,
    pub vel: Vec3,
    pub age: f32,
    pub life: f32,
    pub size: f32,
    /// Size change per second.
    pub grow: f32,
    pub color: [u8; 4],
    pub shape: Shape,
    /// Response time to the wind (s) and fall speed relative to the air (m/s).
    pub tau: f32,
    pub fall: f32,
    /// The tornado whose field it follows (0: the ambient field), and for funnel cloud the
    /// condensation check.
    pub tornado: u32,
    pub condensed: bool,
    pub seed: u8,
}

/// The categories budgets are split between.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Funnel,
    Ground,
    Cloud,
    Precip,
    Burst,
}

impl Category {
    fn share(self) -> f64 {
        match self {
            Category::Funnel => 0.36,
            Category::Ground => 0.22,
            Category::Cloud => 0.14,
            Category::Precip => 0.2,
            Category::Burst => 0.08,
        }
    }
    fn index(self) -> usize {
        self as usize
    }
}

pub struct ParticleSystem {
    pub particles: Vec<Particle>,
    category: Vec<Category>,
    counts: [usize; 5],
    pub budget: usize,
    pub emission: f64,
    pub rng: Rng,
    /// Accumulated fractional emissions per emitter key.
    pub carry: std::collections::HashMap<u64, f64>,
    /// A lightning flash's brightness (0..1), decaying.
    pub flash: f32,
}

impl ParticleSystem {
    pub fn new(quality: Quality, seed: u64) -> Self {
        ParticleSystem {
            particles: Vec::with_capacity(quality.particle_budget()),
            category: Vec::with_capacity(quality.particle_budget()),
            counts: [0; 5],
            budget: quality.particle_budget(),
            emission: quality.emission_scale(),
            rng: Rng::new(seed),
            carry: Default::default(),
            flash: 0.0,
        }
    }

    pub fn set_quality(&mut self, q: Quality) {
        self.budget = q.particle_budget();
        self.emission = q.emission_scale();
        while self.particles.len() > self.budget {
            self.kill(self.particles.len() - 1);
        }
    }

    pub fn len(&self) -> usize {
        self.particles.len()
    }
    pub fn is_empty(&self) -> bool {
        self.particles.is_empty()
    }

    /// Room left in a category.
    pub fn room(&self, c: Category) -> usize {
        let cap = (self.budget as f64 * c.share()) as usize;
        cap.saturating_sub(self.counts[c.index()]).min(self.budget.saturating_sub(self.particles.len()))
    }

    pub fn spawn(&mut self, c: Category, p: Particle) -> bool {
        if self.room(c) == 0 {
            return false;
        }
        self.particles.push(p);
        self.category.push(c);
        self.counts[c.index()] += 1;
        true
    }

    fn kill(&mut self, i: usize) {
        let c = self.category[i];
        self.counts[c.index()] -= 1;
        self.particles.swap_remove(i);
        self.category.swap_remove(i);
    }

    /// How many to emit this step for an emitter `key` at `rate` per second (fractions carry).
    pub fn quota(&mut self, key: u64, rate: f64, dt: f64) -> usize {
        let c = self.carry.entry(key).or_insert(0.0);
        *c += rate * dt * self.emission;
        let n = c.floor();
        *c -= n;
        n as usize
    }

    /// Advance all particles by `dt` in the weather `sys`.
    pub fn update(&mut self, sys: &StormSystem, dt: f64, camera: Vec3) {
        self.flash = (self.flash - dt as f32 * 4.0).max(0.0);
        let mut i = 0;
        while i < self.particles.len() {
            let p = &mut self.particles[i];
            p.age += dt as f32;
            if p.shape == Shape::Line {
                // a line's velocity is its extent; it does not move
                if p.age >= p.life {
                    self.kill(i);
                } else {
                    i += 1;
                }
                continue;
            }
            let far = (p.pos - camera).horizontal_length() > 700.0;
            // the air's motion where it is (each tornado's own field for its particles)
            let air = if p.tornado != 0 {
                match sys.tornadoes.iter().find(|t| t.id == p.tornado) {
                    Some(t) => {
                        let w = t.wind_at(p.pos);
                        if p.condensed {
                            // cloud droplets evaporate outside the condensation funnel
                            let h = p.pos.y - t.pos.y;
                            let f = (h / t.cloud_base.max(1.0)).clamp(0.0, 1.0);
                            let levels = &t.funnel.levels;
                            if !levels.is_empty() {
                                let k = ((f * (levels.len() - 1) as f64).round() as usize).min(levels.len() - 1);
                                let l = levels[k];
                                let axis = t.pos + l.offset;
                                let r = (p.pos - axis).horizontal_length();
                                if r > l.radius * 1.15 + 1.0 {
                                    p.age += dt as f32 * 3.0;
                                }
                            }
                        }
                        w
                    }
                    None => {
                        p.age += dt as f32 * 2.0;
                        sys.env.wind((p.pos.y - sys.ground_y).max(1.0))
                    }
                }
            } else if far || p.shape == Shape::Puff && p.tau > 5.0 {
                // distant and slow things: the storm's motion carries cloud bases
                p.vel
            } else {
                sys.env.wind((p.pos.y - sys.ground_y).max(1.0))
            };
            let target = Vec3::new(air.x, air.y - p.fall as f64, air.z);
            let k = (dt / p.tau.max(0.02) as f64).min(1.0);
            p.vel += (target - p.vel) * k;
            p.pos += p.vel * dt;
            p.size = (p.size + p.grow * dt as f32).max(0.05);
            let below = p.pos.y < sys.ground_y - 2.0 && p.fall > 0.5;
            if p.age >= p.life || below || !p.pos.is_finite() {
                self.kill(i);
            } else {
                i += 1;
            }
        }
    }

    /// Instances for drawing, back to front from `camera`, positions relative to `anchor`.
    pub fn instances(&self, camera: Vec3, anchor: Vec3, light: f32, out: &mut Vec<Instance>) {
        let start = out.len();
        for p in &self.particles {
            let t = (p.age / p.life.max(1e-3)).clamp(0.0, 1.0);
            // fade in quickly, out slowly
            let fade = (t / 0.12).min(1.0) * (1.0 - ((t - 0.6) / 0.4).max(0.0));
            let a = (p.color[3] as f32 * fade) as u8;
            if a == 0 {
                continue;
            }
            let d = p.pos - camera;
            // distant particles drawn a bit larger so the shape survives fewer of them
            let lod = 1.0 + ((d.length() as f32 - 300.0) / 900.0).clamp(0.0, 0.8);
            let shade = |c: u8| ((c as f32) * light.clamp(0.05, 1.5)).min(255.0) as u8;
            out.push(Instance {
                pos: [(p.pos.x - anchor.x) as f32, (p.pos.y - anchor.y) as f32, (p.pos.z - anchor.z) as f32],
                size: p.size * lod,
                vel: [p.vel.x as f32, p.vel.y as f32, p.vel.z as f32],
                color: [shade(p.color[0]), shade(p.color[1]), shade(p.color[2]), a],
                shape: p.shape as u32 | (p.seed as u32) << 8,
            });
        }
        let cam = [(camera.x - anchor.x) as f32, (camera.y - anchor.y) as f32, (camera.z - anchor.z) as f32];
        let dist = |i: &Instance| (i.pos[0] - cam[0]).powi(2) + (i.pos[1] - cam[1]).powi(2) + (i.pos[2] - cam[2]).powi(2);
        out[start..].sort_unstable_by(|a, b| dist(b).total_cmp(&dist(a)));
    }

    pub fn clear(&mut self) {
        self.particles.clear();
        self.category.clear();
        self.counts = [0; 5];
    }
}

/// An approximate colour of a block (its texture's average), for bursts and debris.
pub fn block_color(state: &str) -> [u8; 4] {
    let n = state.split('[').next().unwrap_or(state);
    let n = n.rsplit(':').next().unwrap_or(n);
    let has = |s: &str| n.contains(s);
    if has("leaves") {
        if has("birch") {
            [110, 150, 70, 255]
        } else if has("spruce") {
            [60, 95, 60, 255]
        } else if has("cherry") {
            [230, 170, 200, 255]
        } else {
            [65, 115, 40, 255]
        }
    } else if has("glass") {
        [200, 225, 235, 150]
    } else if has("birch") {
        [205, 190, 140, 255]
    } else if has("dark_oak") {
        [70, 50, 30, 255]
    } else if has("spruce") {
        [110, 80, 50, 255]
    } else if has("stripped") {
        [190, 150, 100, 255]
    } else if has("_log") || has("planks") || has("oak") || has("wood") || has("_door") || has("fence") {
        [150, 112, 70, 255]
    } else if has("brick") {
        [150, 72, 58, 255]
    } else if has("grass_block") || has("short_grass") || has("fern") {
        [95, 140, 60, 255]
    } else if has("dirt") || has("mud") || has("farmland") || has("podzol") {
        [115, 84, 58, 255]
    } else if has("sand") {
        [215, 200, 150, 255]
    } else if has("gravel") {
        [130, 125, 120, 255]
    } else if has("wool") || has("snow") {
        [235, 235, 235, 255]
    } else if has("hay") {
        [200, 170, 60, 255]
    } else if has("stone") || has("cobble") || has("deepslate") || has("andesite") {
        [125, 125, 125, 255]
    } else if has("concrete") {
        [160, 160, 165, 255]
    } else if has("poppy") || has("rose") || has("tulip") {
        [190, 50, 40, 255]
    } else if has("dandelion") || has("sunflower") {
        [230, 210, 50, 255]
    } else {
        [128, 128, 128, 255]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn puff(at: Vec3) -> Particle {
        Particle { pos: at, vel: Vec3::ZERO, age: 0.0, life: 2.0, size: 1.0, grow: 0.0, color: [200, 200, 200, 200], shape: Shape::Puff, tau: 0.1, fall: 0.0, tornado: 0, condensed: false, seed: 0 }
    }

    #[test]
    fn budget_and_categories_cap_particles() {
        let mut s = ParticleSystem::new(Quality::Low, 1);
        let mut n = 0;
        while s.spawn(Category::Funnel, puff(Vec3::ZERO)) {
            n += 1;
        }
        assert_eq!(n, (Quality::Low.particle_budget() as f64 * 0.36) as usize);
        assert!(s.spawn(Category::Precip, puff(Vec3::ZERO)));
    }

    #[test]
    fn particles_follow_the_air_and_expire() {
        let sys = StormSystem::new(crate::atmosphere::Environment::from_preset(crate::atmosphere::presets::Preset::Classic), crate::config::WorldScale::default(), 1);
        let mut s = ParticleSystem::new(Quality::High, 1);
        s.spawn(Category::Cloud, puff(Vec3::new(0.0, 80.0, 0.0)));
        for _ in 0..10 {
            s.update(&sys, 0.05, Vec3::ZERO);
        }
        let v = s.particles[0].vel;
        let air = sys.env.wind(16.0);
        assert!((v - air).length() < 1.0, "{v:?} {air:?}");
        for _ in 0..100 {
            s.update(&sys, 0.05, Vec3::ZERO);
        }
        assert!(s.is_empty());
    }

    #[test]
    fn instances_are_sorted_back_to_front() {
        let mut s = ParticleSystem::new(Quality::High, 1);
        for i in 0..10 {
            let mut p = puff(Vec3::new(i as f64 * 10.0, 64.0, 0.0));
            p.age = 0.5;
            s.spawn(Category::Cloud, p);
        }
        let mut out = Vec::new();
        s.instances(Vec3::ZERO, Vec3::ZERO, 1.0, &mut out);
        assert_eq!(out.len(), 10);
        assert!(out[0].pos[0] > out[9].pos[0]);
    }
}
