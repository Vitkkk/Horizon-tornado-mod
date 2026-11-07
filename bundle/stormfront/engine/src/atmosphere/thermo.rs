// Moist thermodynamics: saturation, mixing ratios, LCL (Bolton 1980) and the pseudo-adiabatic
// lapse rate. Temperatures in kelvin, pressures in pascals, heights in metres.

use crate::units::{CP, EPSILON, G, KAPPA, LV, RD, T0};

/// Saturation vapour pressure over water (Pa), Bolton (1980).
#[inline]
pub fn saturation_vapor_pressure(t: f64) -> f64 {
    let tc = t - T0;
    611.2 * (17.67 * tc / (tc + 243.5)).exp()
}

/// Mixing ratio (kg/kg) for vapour pressure `e` at pressure `p`.
#[inline]
pub fn mixing_ratio(e: f64, p: f64) -> f64 {
    EPSILON * e / (p - e).max(1.0)
}

/// Saturation mixing ratio at temperature `t` and pressure `p`.
#[inline]
pub fn saturation_mixing_ratio(t: f64, p: f64) -> f64 {
    mixing_ratio(saturation_vapor_pressure(t), p)
}

/// Dewpoint (K) of air with vapour pressure `e` (Pa): Bolton's formula inverted.
pub fn dewpoint_from_vapor_pressure(e: f64) -> f64 {
    let l = (e.max(1e-3) / 611.2).ln();
    T0 + 243.5 * l / (17.67 - l)
}

/// Relative humidity (0..1) from temperature and dewpoint.
pub fn relative_humidity(t: f64, td: f64) -> f64 {
    (saturation_vapor_pressure(td) / saturation_vapor_pressure(t)).clamp(0.0, 1.0)
}

/// Virtual temperature.
#[inline]
pub fn virtual_temperature(t: f64, r: f64) -> f64 {
    t * (1.0 + r / EPSILON) / (1.0 + r)
}

/// Potential temperature.
#[inline]
pub fn potential_temperature(t: f64, p: f64) -> f64 {
    t * (100_000.0 / p).powf(KAPPA)
}

/// Temperature at the lifting condensation level, Bolton (1980) eq. 15.
pub fn lcl_temperature(t: f64, td: f64) -> f64 {
    let td = td.min(t);
    1.0 / (1.0 / (td - 56.0) + (t / td).ln() / 800.0) + 56.0
}

/// Height of the LCL above the parcel's start (m): dry-adiabatic ascent to the LCL temperature.
pub fn lcl_height(t: f64, td: f64) -> f64 {
    ((t - lcl_temperature(t, td)) * CP / G).max(0.0)
}

/// Pseudo-adiabatic (saturated) lapse rate (K/m) at temperature `t` and pressure `p`.
pub fn moist_lapse_rate(t: f64, p: f64) -> f64 {
    let rs = saturation_mixing_ratio(t, p);
    let num = G * (1.0 + LV * rs / (RD * t));
    let den = CP + LV * LV * rs * EPSILON / (RD * t * t);
    num / den
}

/// Dry-adiabatic lapse rate (K/m).
pub const DRY_LAPSE: f64 = G / CP;

/// Equivalent potential temperature (K), Bolton (1980) eq. 43 (approximate).
pub fn theta_e(t: f64, td: f64, p: f64) -> f64 {
    let r = saturation_mixing_ratio(td, p);
    let tl = lcl_temperature(t, td);
    let theta = t * (100_000.0 / p).powf(0.2854 * (1.0 - 0.28 * r));
    theta * ((3376.0 / tl - 2.54) * r * (1.0 + 0.81 * r)).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturation_at_20c_is_about_2338_pa() {
        let es = saturation_vapor_pressure(T0 + 20.0);
        assert!((es - 2338.0).abs() < 15.0, "{es}");
    }

    #[test]
    fn dewpoint_inverts_vapor_pressure() {
        for c in [-10.0, 0.0, 12.0, 24.0] {
            let t = T0 + c;
            let back = dewpoint_from_vapor_pressure(saturation_vapor_pressure(t));
            assert!((back - t).abs() < 1e-6);
        }
    }

    #[test]
    fn lcl_height_follows_the_125_m_per_degree_rule() {
        // Espy's rule: about 125 m per degree of dewpoint depression.
        let h = lcl_height(T0 + 30.0, T0 + 20.0);
        assert!((h - 1250.0).abs() < 120.0, "{h}");
        assert!(lcl_height(T0 + 25.0, T0 + 25.0) < 1.0);
    }

    #[test]
    fn moist_lapse_rate_is_between_4_and_9_k_per_km() {
        let warm = moist_lapse_rate(T0 + 25.0, 95_000.0) * 1000.0;
        let cold = moist_lapse_rate(T0 - 20.0, 50_000.0) * 1000.0;
        assert!((3.5..5.0).contains(&warm), "{warm}");
        assert!((7.0..9.8).contains(&cold), "{cold}");
    }
}
