// Radial velocity: the scatterers' motion projected on the beam, never the total wind.
//
// Each scatterer type moves with the air plus its own fall speed (rain ~6 m/s, snow ~1 m/s,
// hail ~15 m/s; debris lags the air and falls); the radar measures the reflectivity-weighted
// mean of their components along the beam. Positive is away from the radar (outbound, red),
// negative towards it (inbound, green). Values beyond the Nyquist velocity fold over.

use super::reflectivity::Scatter;
use crate::math::{Vec3, smoothstep};
use crate::storms::system::StormSystem;

/// Fall speeds (m/s).
pub const FALL_RAIN: f64 = 6.0;
pub const FALL_SNOW: f64 = 1.0;
pub const FALL_HAIL: f64 = 15.0;
pub const FALL_DEBRIS: f64 = 8.0;

/// The air's motion at world `at`, physical height `h`: the storm-scale field, with tornado
/// flow (evaluated at the height it occupies in the world) replacing it near their cores.
pub fn air_motion(sys: &StormSystem, at: Vec3, h: f64) -> Vec3 {
    let ambient = match sys.storm_near(at) {
        Some(s) => s.wind(&sys.env, at, h),
        None => sys.env.wind(h),
    };
    let mut own = Vec3::ZERO;
    let mut weight: f64 = 0.0;
    for t in &sys.tornadoes {
        let d = (at - t.pos).horizontal_length();
        let inner = 3.0 * t.params.r_max;
        let outer = t.influence_radius().max(inner * 1.5);
        if d >= outer {
            continue;
        }
        // physical height to the tornado's world height (the cloud base display mapping)
        let zt = h * t.cloud_base / t.lcl.max(100.0);
        if zt > t.cloud_base * 1.3 {
            continue;
        }
        own += t.wind_at(Vec3::new(at.x, t.pos.y + zt, at.z));
        weight = weight.max(1.0 - smoothstep(inner, outer, d));
    }
    ambient * (1.0 - weight) + own
}

/// Reflectivity-weighted radial velocity (m/s) for unit beam direction `beam`.
pub fn radial_velocity(air: Vec3, z: &Scatter, beam: Vec3) -> f64 {
    let total = z.total();
    if total <= 0.0 {
        return air.dot(beam);
    }
    let fall = |vt: f64| Vec3::new(air.x, air.y - vt, air.z).dot(beam);
    let debris = Vec3::new(air.x * 0.8, air.y * 0.6 - FALL_DEBRIS, air.z * 0.8).dot(beam);
    (z.rain * fall(FALL_RAIN) + z.snow * fall(FALL_SNOW) + z.hail * fall(FALL_HAIL) + z.debris * debris) / total
}

/// Fold `v` into [-nyquist, nyquist).
pub fn alias(v: f64, nyquist: f64) -> f64 {
    if nyquist <= 0.0 {
        return v;
    }
    let span = 2.0 * nyquist;
    (v + nyquist).rem_euclid(span) - nyquist
}

/// A rotation signature found in a velocity sweep.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Couplet {
    /// World position (centre between the two gates).
    pub pos: Vec3,
    /// Gate-to-gate velocity difference (m/s) and rotational velocity (m/s, half of the
    /// difference of the extremes across the couplet).
    pub delta_v: f64,
    pub v_rot: f64,
    /// Diameter between the extremes (blocks).
    pub diameter: f64,
    /// TVS: tight, strong gate-to-gate shear; else a mesocyclone.
    pub tvs: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliasing_folds_into_the_nyquist_interval() {
        assert!((alias(30.0, 25.0) - (-20.0)).abs() < 1e-9);
        assert!((alias(-30.0, 25.0) - 20.0).abs() < 1e-9);
        assert!((alias(10.0, 25.0) - 10.0).abs() < 1e-9);
        assert_eq!(alias(77.0, 0.0), 77.0);
    }

    #[test]
    fn radar_sees_only_the_beam_component() {
        let air = Vec3::new(30.0, 0.0, 0.0);
        let z = Scatter { rain: 1.0, ..Default::default() };
        // beam pointing north (-z): a wind blowing east is invisible
        let v = radial_velocity(air, &z, Vec3::new(0.0, 0.0, -1.0));
        assert!(v.abs() < 1e-9);
        // beam pointing east: all of it, outbound
        let v = radial_velocity(air, &z, Vec3::new(1.0, 0.0, 0.0));
        assert!((v - 30.0).abs() < 1e-9);
        // a beam tilted up sees the fall speed
        let up = Vec3::new(0.0, 1.0, 0.0);
        assert!((radial_velocity(Vec3::ZERO, &z, up) + FALL_RAIN).abs() < 1e-9);
    }
}
