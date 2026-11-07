// Equivalent reflectivity factor from the simulated scatterers.
//
// Rain uses the Marshall-Palmer-based relation Z = 3.63e9 (rho q)^1.75 (rho q in kg/m^3, Z in
// mm^6/m^3); dry snow scatters about 7 dB less for the same water content (dielectric factor
// 0.176/0.93); hail's large particles give far more; tornadic debris is a non-Rayleigh
// scatterer whose echo is taken proportional to its mass concentration. Path-integrated
// attenuation is applied by the scanner per band.

use crate::math::Vec3;
use crate::storms::fields::{FREEZING_LEVEL, Hydrometeors};
use crate::storms::system::StormSystem;

/// Linear reflectivity contributions of one resolution volume (mm^6/m^3).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scatter {
    pub rain: f64,
    pub snow: f64,
    pub hail: f64,
    pub debris: f64,
}

impl Scatter {
    pub fn total(&self) -> f64 {
        self.rain + self.snow + self.hail + self.debris
    }
    pub fn add_weighted(&mut self, o: &Scatter, w: f64) {
        self.rain += o.rain * w;
        self.snow += o.snow * w;
        self.hail += o.hail * w;
        self.debris += o.debris * w;
    }
}

#[inline]
pub fn to_dbz(z: f64) -> f64 {
    10.0 * z.max(1e-10).log10()
}
#[inline]
pub fn from_dbz(dbz: f64) -> f64 {
    10f64.powf(dbz / 10.0)
}

/// Linear Z from hydrometeor contents (g/m^3) at physical height `h`.
pub fn hydrometeor_z(q: &Hydrometeors, h: f64) -> Scatter {
    let rain = if q.rain > 0.0 { 3.63e9 * (q.rain * 1e-3).powf(1.75) } else { 0.0 };
    let mut snow = if q.snow > 0.0 { 0.19 * 3.63e9 * (q.snow * 1e-3).powf(1.75) } else { 0.0 };
    // the melting layer's bright band: wet snowflakes scatter like big raindrops
    let bb = (-((h - (FREEZING_LEVEL - 150.0)) / 220.0).powi(2)).exp();
    snow *= 1.0 + 5.0 * bb;
    let hail = if q.hail > 0.0 { 4.0e10 * (q.hail * 1e-3).powf(1.75) } else { 0.0 };
    Scatter { rain, snow, hail, debris: 0.0 }
}

/// Debris lofted by tornadoes at world `at`, physical height `h`: (mass concentration
/// kg/m^3, linear Z).
pub fn debris_at(sys: &StormSystem, at: Vec3, h: f64) -> (f64, f64) {
    let mut c = 0.0;
    for t in &sys.tornadoes {
        if t.debris_load <= 1.0 {
            continue;
        }
        let r2 = (at - t.pos).horizontal().length_sq();
        let sigma = (1.3 * t.params.r_max).max(40.0);
        if r2 > (5.0 * sigma).powi(2) {
            continue;
        }
        // stronger vortices loft debris higher (TDS heights reach 2-3 km in violent tornadoes)
        let scale_h = 120.0 + 9.0 * t.params.v_max;
        let vol = 2.0 * std::f64::consts::PI * sigma * sigma * scale_h;
        c += t.debris_load / vol * (-r2 / (2.0 * sigma * sigma)).exp() * (-h / scale_h).exp();
    }
    (c, 1.2e9 * c)
}

/// All scatterers at a point.
pub fn scatter_at(sys: &StormSystem, at: Vec3, h: f64) -> Scatter {
    let mut q = Hydrometeors::default();
    for s in &sys.storms {
        q.add(s.hydrometeors(at, h));
    }
    let mut z = hydrometeor_z(&q, h);
    z.debris = debris_at(sys, at, h).1;
    z
}

/// Minimum detectable reflectivity (dBZ) at physical range `r` (m) for a WSR-88D-like radar.
pub fn noise_floor(r: f64) -> f64 {
    -23.0 + 20.0 * (r.max(1000.0) / 1000.0).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rain_content_maps_to_plausible_dbz() {
        let z1 = to_dbz(hydrometeor_z(&Hydrometeors { rain: 1.0, ..Default::default() }, 500.0).rain);
        let z5 = to_dbz(hydrometeor_z(&Hydrometeors { rain: 5.0, ..Default::default() }, 500.0).rain);
        assert!((40.0..46.0).contains(&z1), "{z1}");
        assert!((50.0..58.0).contains(&z5), "{z5}");
        let hail = to_dbz(hydrometeor_z(&Hydrometeors { hail: 3.0, ..Default::default() }, 2000.0).hail);
        assert!(hail > 58.0, "{hail}");
        let snow = to_dbz(hydrometeor_z(&Hydrometeors { snow: 1.0, ..Default::default() }, 8000.0).snow);
        assert!(snow < z1 - 5.0);
    }

    #[test]
    fn noise_floor_rises_with_range() {
        assert!(noise_floor(100_000.0) > noise_floor(10_000.0) + 19.0);
    }
}
