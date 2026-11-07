// The tornado's three-dimensional wind field.
//
// Tangential wind: a Burgers-Rott profile (smooth core, 1/r decay) or a modified Rankine
// profile (solid-body core, r^-alpha decay), scaled near the ground by a power-law surface
// layer. Radial and vertical wind come from an axisymmetric Stokes stream function
//
//     psi(r, z) = A * F(r) * G(z),   F = rc^2 [ (1 - e^-y) - beta * y * e^-y ],  y = r^2 / rc^2
//                                    G = 1 - e^(-z / delta)
//
// so that u_r = -(1/r) dpsi/dz and w = (1/r) dpsi/dr satisfy mass continuity exactly:
// a convergent inflow jet confined to the lowest `delta` metres, feeding an updraft in the
// core (beta = 0, one-celled, as Burgers-Rott) or an annular updraft around a central
// downdraft (beta > 1, two-celled, as in high-swirl-ratio and multiple-vortex tornadoes).
// Everything here is relative to the vortex; translation is added by the caller.

use crate::math::Vec3;
use std::sync::OnceLock;

/// Peak of (1 - e^-x) / sqrt(x), reached at x = 1.25643 (r = 1.12091 rc).
pub const BR_PEAK: f64 = 0.638_190_7;
/// r_max / rc of the Burgers-Rott profile.
pub const BR_RMAX_OVER_RC: f64 = 1.120_906;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RadialProfile {
    BurgersRott,
    /// Modified Rankine with decay exponent `alpha` outside the core (0.5..1).
    ModifiedRankine { alpha: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VortexParams {
    /// Peak tangential wind relative to the vortex, above the surface layer (m/s).
    pub v_max: f64,
    /// Radius of the peak tangential wind (m).
    pub r_max: f64,
    pub profile: RadialProfile,
    /// Peak near-surface inflow as a fraction of `v_max`.
    pub inflow_ratio: f64,
    /// Depth of the inflow layer (m).
    pub inflow_depth: f64,
    /// Cell structure: 0 one-celled; > 1 two-celled with a central downdraft.
    pub beta: f64,
    /// Height where the tangential wind reaches its free value (m); below it a power law.
    pub surface_layer: f64,
}

impl VortexParams {
    pub fn new(v_max: f64, r_max: f64) -> Self {
        let mut p = VortexParams {
            v_max,
            r_max,
            profile: RadialProfile::BurgersRott,
            inflow_ratio: 0.4,
            inflow_depth: 0.0,
            beta: 0.0,
            surface_layer: 0.0,
        };
        p.derive_layers();
        p
    }

    /// Default inflow depth and surface layer from the core size.
    pub fn derive_layers(&mut self) {
        self.inflow_depth = (0.35 * self.r_max).clamp(4.0, 120.0);
        self.surface_layer = (0.25 * self.r_max).clamp(8.0, 60.0);
    }

    /// Core radius rc of the Burgers-Rott profile.
    #[inline]
    pub fn rc(&self) -> f64 {
        (self.r_max / BR_RMAX_OVER_RC).max(0.5)
    }

    /// Tangential wind (m/s) at radius `r`, above the surface layer.
    pub fn tangential(&self, r: f64) -> f64 {
        let r = r.max(1e-6);
        match self.profile {
            RadialProfile::BurgersRott => {
                let x = (r / self.rc()).powi(2);
                self.v_max * (1.0 - (-x).exp()) / x.sqrt() / BR_PEAK
            }
            RadialProfile::ModifiedRankine { alpha } => {
                let s = r / self.r_max;
                if s <= 1.0 { self.v_max * s } else { self.v_max * s.powf(-alpha) }
            }
        }
    }

    /// Fraction of the free tangential wind at height `z` (m) above the ground.
    #[inline]
    pub fn surface_factor(&self, z: f64) -> f64 {
        let zm = self.surface_layer.max(1.0);
        if z >= zm { 1.0 } else { (z.max(0.25) / zm).powf(0.11) }
    }

    /// Stream-function amplitude A from the target inflow.
    fn amplitude(&self) -> f64 {
        let u_in = self.inflow_ratio * self.v_max;
        u_in * self.inflow_depth.max(1.0) / (BR_PEAK * self.rc())
    }

    /// Radial (positive outward) and vertical wind (m/s) at radius `r`, height `z`.
    pub fn radial_vertical(&self, r: f64, z: f64) -> (f64, f64) {
        let rc = self.rc();
        let a = self.amplitude();
        let delta = self.inflow_depth.max(1.0);
        let y = (r / rc).powi(2);
        let ey = (-y).exp();
        let ez = (-z.max(0.0) / delta).exp();
        let f = rc * rc * ((1.0 - ey) - self.beta * y * ey);
        let ur = if r > 1e-6 { -(a / (r * delta)) * f * ez } else { 0.0 };
        let w = 2.0 * a * ey * (1.0 - self.beta + self.beta * y) * (1.0 - ez);
        (ur, w)
    }

    /// The vortex-relative wind at `offset` (horizontal offset from the axis, `y` = height
    /// above the ground in metres).
    pub fn wind(&self, offset: Vec3) -> Vec3 {
        let horiz = offset.horizontal();
        let r = horiz.length();
        let z = offset.y;
        let vt = self.tangential(r) * self.surface_factor(z);
        let (ur, w) = self.radial_vertical(r, z);
        if r < 1e-6 {
            return Vec3::new(0.0, w, 0.0);
        }
        let rhat = horiz / r;
        rhat.cyclonic_tangent() * vt + rhat * ur + Vec3::new(0.0, w, 0.0)
    }

    /// Cyclostrophic pressure deficit (Pa) at radius `r` above the surface layer:
    /// dp/dr = rho V^2 / r integrated from r to infinity.
    pub fn pressure_deficit(&self, r: f64, rho: f64) -> f64 {
        let q = rho * self.v_max * self.v_max;
        match self.profile {
            RadialProfile::BurgersRott => q * br_deficit_table(r / self.rc()),
            RadialProfile::ModifiedRankine { alpha } => {
                let s = r / self.r_max;
                if s >= 1.0 {
                    q * s.powf(-2.0 * alpha) / (2.0 * alpha)
                } else {
                    q * (1.0 / (2.0 * alpha) + 0.5 * (1.0 - s * s))
                }
            }
        }
    }

    /// The radius (m) where the pressure deficit equals `dp`, or None when even the centre's
    /// deficit is smaller.
    pub fn radius_of_deficit(&self, dp: f64, rho: f64) -> Option<f64> {
        if dp <= 0.0 {
            return Some(f64::INFINITY);
        }
        if self.pressure_deficit(0.0, rho) < dp {
            return None;
        }
        // bisection on a monotonic function
        let (mut lo, mut hi) = (0.0, self.r_max.max(1.0));
        while self.pressure_deficit(hi, rho) > dp {
            hi *= 2.0;
            if hi > 1e6 {
                return Some(hi);
            }
        }
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            if self.pressure_deficit(mid, rho) > dp { lo = mid } else { hi = mid }
        }
        Some(0.5 * (lo + hi))
    }

    /// Outer radius (m) of winds at or above `threshold` (vortex-relative tangential plus a
    /// translation `trans` on the strong side), or 0 when the vortex never reaches it.
    pub fn radius_of_wind(&self, threshold: f64, trans: f64) -> f64 {
        if self.v_max + trans < threshold {
            return 0.0;
        }
        let (mut lo, mut hi) = (self.r_max, self.r_max * 2.0);
        while self.tangential(hi) + trans >= threshold {
            hi *= 2.0;
            if hi > 50_000.0 {
                return hi;
            }
        }
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            if self.tangential(mid) + trans >= threshold { lo = mid } else { hi = mid }
        }
        lo
    }
}

/// Dimensionless Burgers-Rott pressure deficit dp / (rho Vmax^2) at s = r / rc, tabulated once.
fn br_deficit_table(s: f64) -> f64 {
    const N: usize = 2048;
    const S_MAX: f64 = 32.0;
    static TABLE: OnceLock<Vec<f64>> = OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let ds = S_MAX / N as f64;
        let f2_over_s = |s: f64| -> f64 {
            if s < 1e-9 {
                return 0.0;
            }
            let x = s * s;
            let v = (1.0 - (-x).exp()) / s / BR_PEAK;
            v * v / s
        };
        // tail beyond S_MAX: V ~ 1/(BR_PEAK s) so the integral is 1 / (2 BR_PEAK^2 s^2)
        let mut out = vec![0.0; N + 1];
        out[N] = 1.0 / (2.0 * BR_PEAK * BR_PEAK * S_MAX * S_MAX);
        for i in (0..N).rev() {
            let s0 = i as f64 * ds;
            let s1 = s0 + ds;
            // Simpson on each cell
            let mid = f2_over_s(0.5 * (s0 + s1));
            out[i] = out[i + 1] + ds / 6.0 * (f2_over_s(s0) + 4.0 * mid + f2_over_s(s1));
        }
        out
    });
    if s >= S_MAX {
        return 1.0 / (2.0 * BR_PEAK * BR_PEAK * s * s);
    }
    let x = s.max(0.0) / S_MAX * N as f64;
    let i = (x as usize).min(N - 1);
    let f = x - i as f64;
    t[i] * (1.0 - f) + t[i + 1] * f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burgers_rott_peaks_at_r_max() {
        let p = VortexParams::new(80.0, 50.0);
        let mut best = (0.0, 0.0);
        for i in 1..2000 {
            let r = i as f64 * 0.1;
            let v = p.tangential(r);
            if v > best.1 {
                best = (r, v);
            }
        }
        assert!((best.0 - 50.0).abs() < 0.5, "{best:?}");
        assert!((best.1 - 80.0).abs() < 0.05, "{best:?}");
        // decays as 1/r far away
        let far = p.tangential(1000.0) * 1000.0;
        let farther = p.tangential(2000.0) * 2000.0;
        assert!((far - farther).abs() / far < 1e-3);
    }

    #[test]
    fn modified_rankine_shape() {
        let mut p = VortexParams::new(60.0, 30.0);
        p.profile = RadialProfile::ModifiedRankine { alpha: 0.6 };
        assert!((p.tangential(15.0) - 30.0).abs() < 1e-9);
        assert!((p.tangential(30.0) - 60.0).abs() < 1e-9);
        assert!((p.tangential(60.0) - 60.0 * 2f64.powf(-0.6)).abs() < 1e-9);
    }

    #[test]
    fn rankine_deficit_matches_closed_form_at_centre() {
        let mut p = VortexParams::new(50.0, 40.0);
        p.profile = RadialProfile::ModifiedRankine { alpha: 1.0 };
        // classic Rankine: centre deficit rho V^2
        assert!((p.pressure_deficit(0.0, 1.2) - 1.2 * 2500.0).abs() < 1e-6);
    }

    #[test]
    fn burgers_rott_deficit_is_monotonic_and_matches_numeric_integral() {
        let p = VortexParams::new(70.0, 60.0);
        let rho = 1.15;
        let mut last = f64::MAX;
        for i in 0..200 {
            let d = p.pressure_deficit(i as f64 * 5.0, rho);
            assert!(d <= last + 1e-9);
            last = d;
        }
        // brute-force integral at r = r_max: dp = integral of rho V^2 d(ln r)
        let (l0, l1) = (p.r_max.ln(), 1e7f64.ln());
        let n = 200_000;
        let dl = (l1 - l0) / n as f64;
        let mut sum = 0.0;
        for i in 0..n {
            let v = p.tangential((l0 + (i as f64 + 0.5) * dl).exp());
            sum += rho * v * v * dl;
        }
        let table = p.pressure_deficit(p.r_max, rho);
        assert!((sum - table).abs() / table < 0.002, "{sum} vs {table}");
    }

    #[test]
    fn flow_satisfies_mass_continuity() {
        // (1/r) d(r u_r)/dr + dw/dz = 0, checked by finite differences.
        let mut p = VortexParams::new(80.0, 50.0);
        for beta in [0.0, 1.8] {
            p.beta = beta;
            for &(r, z) in &[(10.0, 3.0), (45.0, 10.0), (90.0, 2.0), (30.0, 40.0)] {
                let h = 1e-3;
                let (u1, _) = p.radial_vertical(r + h, z);
                let (u0, _) = p.radial_vertical(r - h, z);
                let (_, w1) = p.radial_vertical(r, z + h);
                let (_, w0) = p.radial_vertical(r, z - h);
                let div = ((r + h) * u1 - (r - h) * u0) / (2.0 * h * r) + (w1 - w0) / (2.0 * h);
                assert!(div.abs() < 1e-4, "beta {beta} r {r} z {z}: {div}");
            }
        }
    }

    #[test]
    fn inflow_near_ground_and_two_celled_core_downdraft() {
        let mut p = VortexParams::new(80.0, 50.0);
        let (ur, w) = p.radial_vertical(60.0, 1.0);
        assert!(ur < -10.0, "inflow {ur}");
        assert!(w > 0.0);
        let (_, w_core) = p.radial_vertical(0.0, 200.0);
        assert!(w_core > 20.0, "one-celled updraft {w_core}");
        p.beta = 2.0;
        let (_, w_core) = p.radial_vertical(0.0, 200.0);
        assert!(w_core < -10.0, "two-celled downdraft {w_core}");
        let (_, w_ring) = p.radial_vertical(p.rc() * 1.2, 200.0);
        assert!(w_ring > 0.0);
    }

    #[test]
    fn wind_turns_cyclonically_and_weakens_at_ground() {
        let p = VortexParams::new(80.0, 50.0);
        // east of the centre, 30 m up: wind blows mostly north (-z) with inflow (-x)
        let v = p.wind(Vec3::new(50.0, 30.0, 0.0));
        assert!(v.z < -70.0, "{v:?}");
        assert!(v.x < 0.0);
        let low = p.wind(Vec3::new(50.0, 1.0, 0.0));
        assert!(low.horizontal_length() < v.horizontal_length() * 1.05);
    }

    #[test]
    fn deficit_radius_inverts_deficit() {
        let p = VortexParams::new(70.0, 80.0);
        let dp = p.pressure_deficit(55.0, 1.2);
        let r = p.radius_of_deficit(dp, 1.2).unwrap();
        assert!((r - 55.0).abs() < 0.01, "{r}");
        assert!(p.radius_of_deficit(1e9, 1.2).is_none());
    }

    #[test]
    fn damaging_radius_grows_with_translation() {
        let p = VortexParams::new(60.0, 40.0);
        let r0 = p.radius_of_wind(40.0, 0.0);
        let r1 = p.radius_of_wind(40.0, 15.0);
        assert!(r1 > r0 && r0 > 40.0, "{r0} {r1}");
        assert_eq!(p.radius_of_wind(100.0, 10.0), 0.0);
    }
}
