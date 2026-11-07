// Suction (secondary) vortices of a multiple-vortex tornado.
//
// In a two-celled tornado the shear zone around the central downdraft rolls up into a few
// small, intense vortices that orbit the parent at roughly its radius of maximum wind, slower
// than the parent's peak wind, each living tens of seconds. Their winds add to the parent's,
// so the strongest ground-relative winds sit in narrow swaths along their cycloidal tracks.

use crate::math::{Rng, Vec3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SubVortex {
    /// Angle of the orbit (rad, counter-clockwise from +x seen from above).
    pub angle: f64,
    /// Orbit radius (m).
    pub orbit: f64,
    /// Core radius (m).
    pub core: f64,
    /// Peak tangential wind of the subvortex (m/s), at full strength.
    pub v_peak: f64,
    /// Age and lifetime (s).
    pub age: f64,
    pub life: f64,
}

impl SubVortex {
    /// Strength envelope: grows, holds and decays over its life.
    pub fn envelope(&self) -> f64 {
        let t = (self.age / self.life.max(1e-3)).clamp(0.0, 1.0);
        let grow = (t / 0.2).min(1.0);
        let decay = ((1.0 - t) / 0.35).min(1.0);
        (grow * decay).max(0.0)
    }

    /// Centre, relative to the parent's axis (horizontal, y = 0).
    pub fn offset(&self) -> Vec3 {
        Vec3::new(self.orbit * self.angle.cos(), 0.0, -self.orbit * self.angle.sin())
    }

    /// The subvortex's own wind (m/s) at `rel` (offset from the parent's axis, y = height),
    /// relative to its own motion: a Rankine-like core with a strong updraft, confined to the
    /// lowest few hundred metres.
    pub fn wind(&self, rel: Vec3) -> Vec3 {
        let d = (rel - self.offset()).horizontal();
        let r = d.length();
        let s = self.envelope();
        if s <= 0.0 || r > self.core * 8.0 {
            return Vec3::ZERO;
        }
        let x = (r / self.core).powi(2);
        let vt = self.v_peak * s * (1.0 - (-x).exp()) / x.sqrt().max(1e-6) / super::vortex::BR_PEAK;
        let height = (-(rel.y.max(0.0)) / 250.0).exp();
        let near = (rel.y.max(0.3) / 6.0).min(1.0).powf(0.15);
        let w = 0.6 * self.v_peak * s * (-x).exp() * near;
        if r < 1e-6 {
            return Vec3::new(0.0, w, 0.0);
        }
        (d / r).cyclonic_tangent() * (vt * height * near) + Vec3::new(0.0, w, 0.0)
    }
}

/// The set of subvortices of one tornado.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MultiVortex {
    pub enabled: bool,
    pub vortices: Vec<SubVortex>,
    /// Target number of subvortices (2..6), from the swirl ratio.
    pub target: usize,
    spawn_timer: f64,
}

impl MultiVortex {
    pub fn new(enabled: bool) -> Self {
        Self { enabled, vortices: Vec::new(), target: 0, spawn_timer: 0.0 }
    }

    /// Advance by `dt` given the parent's peak wind and radius and its swirl (beta).
    pub fn step(&mut self, rng: &mut Rng, dt: f64, v_max: f64, r_max: f64, beta: f64) {
        if !self.enabled || v_max < 35.0 {
            for v in &mut self.vortices {
                v.age += dt * 3.0;
            }
            self.vortices.retain(|v| v.age < v.life);
            return;
        }
        // more, smaller vortices as the two-celled structure strengthens (swirl ratio)
        self.target = (2.0 + 1.5 * (beta - 1.0).max(0.0) + r_max / 150.0).round().clamp(2.0, 6.0) as usize;
        // orbit about 30-60% slower than the peak wind of the parent
        let omega = 0.55 * v_max / r_max.max(5.0);
        for v in &mut self.vortices {
            v.angle += omega * dt * (v.orbit / r_max.max(1.0)).powf(-0.2);
            v.age += dt;
            // the orbit breathes around the radius of maximum wind
            v.orbit += (r_max * 0.9 - v.orbit) * (dt / 8.0);
        }
        self.vortices.retain(|v| v.age < v.life);
        self.spawn_timer -= dt;
        if self.vortices.len() < self.target && self.spawn_timer <= 0.0 {
            let base_angle = if let Some(last) = self.vortices.last() {
                last.angle + std::f64::consts::TAU / self.target as f64
            } else {
                rng.range(0.0, std::f64::consts::TAU)
            };
            self.vortices.push(SubVortex {
                angle: base_angle + rng.range(-0.3, 0.3),
                orbit: r_max * rng.range(0.75, 1.05),
                core: (r_max * rng.range(0.08, 0.16)).clamp(3.0, 40.0),
                v_peak: v_max * rng.range(0.30, 0.50),
                age: 0.0,
                life: rng.range(8.0, 35.0),
            });
            self.spawn_timer = rng.range(1.0, 4.0);
        }
    }

    /// The summed subvortex wind at `rel` (offset from the parent's axis). Each subvortex
    /// is carried along its orbit, so its translation (the orbital speed) adds on its outer
    /// flank: the parent's own wind already includes that motion.
    pub fn wind(&self, rel: Vec3) -> Vec3 {
        if self.vortices.is_empty() {
            return Vec3::ZERO;
        }
        self.vortices.iter().map(|v| v.wind(rel)).fold(Vec3::ZERO, |a, b| a + b)
    }

    /// Strongest subvortex wind right now (m/s), for HUDs and surveys.
    pub fn peak(&self) -> f64 {
        self.vortices.iter().map(|v| v.v_peak * v.envelope()).fold(0.0, f64::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subvortices_spawn_orbit_and_expire() {
        let mut rng = Rng::new(3);
        let mut mv = MultiVortex::new(true);
        let mut seen = 0;
        for _ in 0..2000 {
            mv.step(&mut rng, 0.05, 80.0, 120.0, 2.0);
            seen = seen.max(mv.vortices.len());
            for v in &mv.vortices {
                assert!(v.orbit > 50.0 && v.orbit < 160.0);
            }
        }
        assert!(seen >= 3, "{seen}");
        // disabled: they decay away
        mv.enabled = false;
        for _ in 0..400 {
            mv.step(&mut rng, 0.05, 80.0, 120.0, 2.0);
        }
        assert!(mv.vortices.is_empty());
    }

    #[test]
    fn subvortex_wind_peaks_near_its_core() {
        let sv = SubVortex { angle: 0.0, orbit: 100.0, core: 10.0, v_peak: 40.0, age: 10.0, life: 30.0 };
        let at_core_edge = sv.wind(Vec3::new(100.0 + 11.2, 10.0, 0.0)).horizontal_length();
        let far = sv.wind(Vec3::new(100.0 + 60.0, 10.0, 0.0)).horizontal_length();
        assert!(at_core_edge > 35.0, "{at_core_edge}");
        // 1/r decay: at six core radii about a quarter of the peak
        assert!(far < 0.3 * at_core_edge, "{far}");
    }
}
