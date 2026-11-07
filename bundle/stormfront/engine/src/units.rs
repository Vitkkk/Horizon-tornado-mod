// Physical constants and unit conversions. Internal units are SI: metres (one block is one
// metre), seconds, m/s, pascals, kelvin, kilograms.

/// Gravity (m/s^2).
pub const G: f64 = 9.80665;
/// Gas constant of dry air (J/kg/K).
pub const RD: f64 = 287.04;
/// Gas constant of water vapour (J/kg/K).
pub const RV: f64 = 461.5;
/// Specific heat of dry air at constant pressure (J/kg/K).
pub const CP: f64 = 1005.7;
/// Latent heat of vaporisation (J/kg).
pub const LV: f64 = 2.501e6;
/// Rd / cp.
pub const KAPPA: f64 = RD / CP;
/// Rd / Rv.
pub const EPSILON: f64 = RD / RV;
/// Near-surface air density used by the damage model (kg/m^3).
pub const RHO_SURFACE: f64 = 1.20;
/// 0 degC in kelvin.
pub const T0: f64 = 273.15;
/// Ticks per second of the game.
pub const TICKS_PER_SECOND: f64 = 20.0;
/// Seconds per game tick.
pub const TICK: f64 = 1.0 / TICKS_PER_SECOND;

#[inline]
pub fn ms_to_kmh(v: f64) -> f64 {
    v * 3.6
}
#[inline]
pub fn kmh_to_ms(v: f64) -> f64 {
    v / 3.6
}
#[inline]
pub fn ms_to_mph(v: f64) -> f64 {
    v * 2.236_936
}
#[inline]
pub fn ms_to_kt(v: f64) -> f64 {
    v * 1.943_844
}
#[inline]
pub fn c_to_k(c: f64) -> f64 {
    c + T0
}
#[inline]
pub fn k_to_c(k: f64) -> f64 {
    k - T0
}
/// Dynamic pressure q = 1/2 rho V^2 (Pa).
#[inline]
pub fn dynamic_pressure(rho: f64, v: f64) -> f64 {
    0.5 * rho * v * v
}
/// Aerodynamic force F = 1/2 rho Cd A V^2 (N).
#[inline]
pub fn drag_force(rho: f64, cd: f64, area: f64, v: f64) -> f64 {
    dynamic_pressure(rho, v) * cd * area
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert!((ms_to_kmh(100.0) - 360.0).abs() < 1e-9);
        assert!((kmh_to_ms(ms_to_kmh(37.5)) - 37.5).abs() < 1e-12);
        assert!((ms_to_mph(44.704) - 100.0).abs() < 1e-3);
    }

    #[test]
    fn dynamic_pressure_of_100_ms_is_6_kpa() {
        let q = dynamic_pressure(RHO_SURFACE, 100.0);
        assert!((q - 6000.0).abs() < 1.0);
    }
}
