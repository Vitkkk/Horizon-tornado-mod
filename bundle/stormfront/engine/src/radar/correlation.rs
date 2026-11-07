// Correlation coefficient (rho_hv): how alike the horizontal and vertical returns are.
//
// Uniform rain scatters coherently in both polarisations (0.98-1.0); snow slightly less; hail
// and mixed phase about 0.90-0.95; tumbling, irregular debris of all sizes and materials
// decorrelates them strongly (often below 0.8). A volume mixing different scatterers is lower
// than the reflectivity-weighted mean of its parts. A tornado debris signature (TDS) is low CC
// with substantial reflectivity, collocated with a velocity couplet: it exists only where the
// simulation actually lofted debris and the beam can resolve it.

use super::reflectivity::Scatter;

pub const CC_RAIN: f64 = 0.992;
pub const CC_SNOW: f64 = 0.975;
pub const CC_HAIL: f64 = 0.925;
pub const CC_DEBRIS: f64 = 0.45;

/// The mixture's correlation coefficient (before noise).
pub fn correlation(z: &Scatter) -> f64 {
    let total = z.total();
    if total <= 0.0 {
        return f64::NAN;
    }
    let mean = (z.rain * CC_RAIN + z.snow * CC_SNOW + z.hail * CC_HAIL + z.debris * CC_DEBRIS) / total;
    // diversity: a volume half debris and half rain decorrelates more than either part
    let parts = [z.rain / total, z.snow / total, z.hail / total, z.debris / total];
    let diversity = 1.0 - parts.iter().map(|p| p * p).sum::<f64>();
    (mean - 0.12 * diversity).clamp(0.05, 1.0)
}

/// Whether a gate's values look like debris: substantial echo with low CC.
pub fn debris_like(dbz: f64, cc: f64) -> bool {
    dbz >= 30.0 && cc <= 0.82
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rain_is_near_one_and_debris_is_low() {
        assert!(correlation(&Scatter { rain: 1e4, ..Default::default() }) > 0.98);
        let tds = correlation(&Scatter { rain: 1e4, debris: 1e5, ..Default::default() });
        assert!(tds < 0.6, "{tds}");
        let hail = correlation(&Scatter { rain: 1e5, hail: 1e6, ..Default::default() });
        assert!((0.85..0.96).contains(&hail), "{hail}");
        assert!(correlation(&Scatter::default()).is_nan());
    }
}
