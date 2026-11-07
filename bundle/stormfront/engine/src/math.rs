// Small math kit: vectors, a deterministic random source and interpolation helpers.
//
// World frame (Minecraft): +x east, +y up, +z south. Meteorological winds are given as
// (u east, v north) and converted with `Vec3::from_met`.

use std::ops::{Add, AddAssign, Div, Mul, MulAssign, Neg, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 { x: 0.0, y: 0.0, z: 0.0 };
    pub const UP: Vec3 = Vec3 { x: 0.0, y: 1.0, z: 0.0 };

    #[inline]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }
    /// A horizontal vector from a meteorological wind (u east, v north) and vertical w.
    #[inline]
    pub fn from_met(u: f64, v: f64, w: f64) -> Self {
        Self::new(u, w, -v)
    }
    /// (u east, v north) of the horizontal part.
    #[inline]
    pub fn met(self) -> (f64, f64) {
        (self.x, -self.z)
    }
    #[inline]
    pub fn from_array(a: [f64; 3]) -> Self {
        Self::new(a[0], a[1], a[2])
    }
    #[inline]
    pub fn to_array(self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }
    #[inline]
    pub fn dot(self, o: Vec3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    #[inline]
    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    #[inline]
    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }
    #[inline]
    pub fn length_sq(self) -> f64 {
        self.dot(self)
    }
    #[inline]
    pub fn horizontal(self) -> Vec3 {
        Vec3::new(self.x, 0.0, self.z)
    }
    #[inline]
    pub fn horizontal_length(self) -> f64 {
        (self.x * self.x + self.z * self.z).sqrt()
    }
    pub fn normalized(self) -> Vec3 {
        let l = self.length();
        if l > 1e-12 { self / l } else { Vec3::ZERO }
    }
    #[inline]
    pub fn lerp(self, o: Vec3, t: f64) -> Vec3 {
        self + (o - self) * t
    }
    #[inline]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
    /// The block holding this point.
    #[inline]
    pub fn block(self) -> [i32; 3] {
        [self.x.floor() as i32, self.y.floor() as i32, self.z.floor() as i32]
    }
    /// Counter-clockwise (cyclonic, northern hemisphere) tangent of the horizontal offset `self`,
    /// seen from above, with the same length.
    #[inline]
    pub fn cyclonic_tangent(self) -> Vec3 {
        Vec3::new(self.z, 0.0, -self.x)
    }
    pub fn clamp_length(self, max: f64) -> Vec3 {
        let l = self.length();
        if l > max && l > 0.0 { self * (max / l) } else { self }
    }
}

impl Add for Vec3 {
    type Output = Vec3;
    #[inline]
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl AddAssign for Vec3 {
    #[inline]
    fn add_assign(&mut self, o: Vec3) {
        *self = *self + o;
    }
}
impl Sub for Vec3 {
    type Output = Vec3;
    #[inline]
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl SubAssign for Vec3 {
    #[inline]
    fn sub_assign(&mut self, o: Vec3) {
        *self = *self - o;
    }
}
impl Mul<f64> for Vec3 {
    type Output = Vec3;
    #[inline]
    fn mul(self, s: f64) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}
impl MulAssign<f64> for Vec3 {
    #[inline]
    fn mul_assign(&mut self, s: f64) {
        *self = *self * s;
    }
}
impl Div<f64> for Vec3 {
    type Output = Vec3;
    #[inline]
    fn div(self, s: f64) -> Vec3 {
        Vec3::new(self.x / s, self.y / s, self.z / s)
    }
}
impl Neg for Vec3 {
    type Output = Vec3;
    #[inline]
    fn neg(self) -> Vec3 {
        Vec3::new(-self.x, -self.y, -self.z)
    }
}

/// A small, fast, deterministic random source (PCG-XSH-RR 64/32 plus helpers). Server and
/// client seed their own; nothing about damage depends on the client's draws.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
    inc: u64,
    spare_normal: Option<f64>,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut r = Rng { state: 0, inc: (seed << 1) | 1, spare_normal: None };
        r.next_u32();
        r.state = r.state.wrapping_add(seed ^ 0x9E37_79B9_7F4A_7C15);
        r.next_u32();
        r
    }
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        ((self.next_u32() as u64) << 32) | self.next_u32() as u64
    }
    /// Uniform in [0, 1).
    #[inline]
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }
    #[inline]
    pub fn f32(&mut self) -> f32 {
        self.f64() as f32
    }
    #[inline]
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.f64()
    }
    /// Uniform integer in [0, n).
    #[inline]
    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 { 0 } else { ((self.next_u32() as u64 * n as u64) >> 32) as u32 }
    }
    #[inline]
    pub fn chance(&mut self, p: f64) -> bool {
        self.f64() < p
    }
    /// Standard normal (Box-Muller, keeps the spare).
    pub fn normal(&mut self) -> f64 {
        if let Some(s) = self.spare_normal.take() {
            return s;
        }
        loop {
            let u = self.range(-1.0, 1.0);
            let v = self.range(-1.0, 1.0);
            let s = u * u + v * v;
            if s > 0.0 && s < 1.0 {
                let m = (-2.0 * s.ln() / s).sqrt();
                self.spare_normal = Some(v * m);
                return u * m;
            }
        }
    }
    /// A uniformly distributed unit vector.
    pub fn unit_vector(&mut self) -> Vec3 {
        loop {
            let v = Vec3::new(self.range(-1.0, 1.0), self.range(-1.0, 1.0), self.range(-1.0, 1.0));
            let l = v.length_sq();
            if l > 1e-6 && l <= 1.0 {
                return v / l.sqrt();
            }
        }
    }
}

/// Hash of integer coordinates to [0, 1): stable per-block randomness (material variation,
/// construction quality) that every side computes alike without storing anything.
#[inline]
pub fn hash01(x: i32, y: i32, z: i32, salt: u32) -> f64 {
    let mut h = (x as u32 as u64).wrapping_mul(0x9E37_79B1)
        ^ (y as u32 as u64).wrapping_mul(0x85EB_CA77).rotate_left(17)
        ^ (z as u32 as u64).wrapping_mul(0xC2B2_AE3D).rotate_left(31)
        ^ (salt as u64).wrapping_mul(0x27D4_EB2F_1656_67C5);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    h = h.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    h ^= h >> 33;
    (h >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

/// An Ornstein-Uhlenbeck process: mean-reverting noise for gusts and intensity pulses.
#[derive(Clone, Debug, Default)]
pub struct OuProcess {
    pub value: f64,
}

impl OuProcess {
    /// Advance by `dt` with relaxation time `tau` (s) and stationary standard deviation `sigma`.
    pub fn step(&mut self, rng: &mut Rng, dt: f64, tau: f64, sigma: f64) -> f64 {
        let a = (-dt / tau.max(1e-6)).exp();
        self.value = self.value * a + sigma * (1.0 - a * a).max(0.0).sqrt() * rng.normal();
        self.value
    }
}

#[inline]
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}
#[inline]
pub fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}
#[inline]
pub fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = clamp01((x - e0) / (e1 - e0));
    t * t * (3.0 - 2.0 * t)
}
/// Linear interpolation in a table of (x, y) sorted by x, clamped at the ends.
pub fn table(points: &[(f64, f64)], x: f64) -> f64 {
    match points {
        [] => 0.0,
        [only] => only.1,
        _ => {
            if x <= points[0].0 {
                return points[0].1;
            }
            for w in points.windows(2) {
                if x <= w[1].0 {
                    let t = (x - w[0].0) / (w[1].0 - w[0].0);
                    return lerp(w[0].1, w[1].1, t);
                }
            }
            points[points.len() - 1].1
        }
    }
}
/// Angle wrapped to (-pi, pi].
#[inline]
pub fn wrap_angle(a: f64) -> f64 {
    let mut a = a % std::f64::consts::TAU;
    if a <= -std::f64::consts::PI {
        a += std::f64::consts::TAU;
    } else if a > std::f64::consts::PI {
        a -= std::f64::consts::TAU;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cyclonic_tangent_turns_counter_clockwise_from_above() {
        // A point east of the centre moves north (-z) in a cyclonic vortex.
        let t = Vec3::new(1.0, 0.0, 0.0).cyclonic_tangent();
        assert_eq!(t, Vec3::new(0.0, 0.0, -1.0));
        // A point north of the centre moves west.
        let t = Vec3::new(0.0, 0.0, -1.0).cyclonic_tangent();
        assert_eq!(t, Vec3::new(-1.0, 0.0, 0.0));
    }

    #[test]
    fn met_round_trip() {
        let v = Vec3::from_met(3.0, 4.0, 1.0);
        assert_eq!(v.met(), (3.0, 4.0));
        assert_eq!(v.y, 1.0);
    }

    #[test]
    fn rng_is_deterministic_and_uniform() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        let mut sum = 0.0;
        for _ in 0..10_000 {
            let x = a.f64();
            assert_eq!(x, b.f64());
            assert!((0.0..1.0).contains(&x));
            sum += x;
        }
        assert!((sum / 10_000.0 - 0.5).abs() < 0.02);
    }

    #[test]
    fn normal_has_unit_variance() {
        let mut r = Rng::new(7);
        let n = 20_000;
        let (mut s, mut s2) = (0.0, 0.0);
        for _ in 0..n {
            let x = r.normal();
            s += x;
            s2 += x * x;
        }
        let mean = s / n as f64;
        let var = s2 / n as f64 - mean * mean;
        assert!(mean.abs() < 0.03, "{mean}");
        assert!((var - 1.0).abs() < 0.05, "{var}");
    }

    #[test]
    fn table_interpolates_and_clamps() {
        let t = [(0.0, 0.0), (10.0, 100.0), (20.0, 50.0)];
        assert_eq!(table(&t, -5.0), 0.0);
        assert_eq!(table(&t, 5.0), 50.0);
        assert_eq!(table(&t, 15.0), 75.0);
        assert_eq!(table(&t, 99.0), 50.0);
    }

    #[test]
    fn hash_is_stable_and_spread() {
        assert_eq!(hash01(1, 2, 3, 4), hash01(1, 2, 3, 4));
        assert_ne!(hash01(1, 2, 3, 4), hash01(1, 2, 4, 4));
        let mean: f64 = (0..1000).map(|i| hash01(i, 64, -i, 9)).sum::<f64>() / 1000.0;
        assert!((mean - 0.5).abs() < 0.05);
    }
}
