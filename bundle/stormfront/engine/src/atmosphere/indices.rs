// Convective parameters computed from a sounding: parcel CAPE/CIN/LCL/LFC/EL, bulk shear,
// Bunkers storm motion, storm-relative helicity and the composite indices forecasters use.

use super::sounding::{DZ, Sounding, TOP};
use super::thermo::{
    DRY_LAPSE, lcl_height, mixing_ratio, moist_lapse_rate, saturation_mixing_ratio,
    saturation_vapor_pressure, virtual_temperature,
};
use crate::units::G;

/// What a lifted parcel does.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ParcelResult {
    /// Convective available potential energy (J/kg).
    pub cape: f64,
    /// Convective inhibition (J/kg, negative or zero).
    pub cin: f64,
    /// Heights above ground (m).
    pub lcl: f64,
    pub lfc: Option<f64>,
    pub el: Option<f64>,
    /// CAPE below 3 km (J/kg): low-level buoyancy, which favours stretching near the ground.
    pub cape_0_3km: f64,
    /// Theoretical updraft speed sqrt(2 CAPE) (m/s).
    pub w_max: f64,
}

/// Lift a parcel of temperature `t0`, dewpoint `td0` from height `z0` through `sounding`.
pub fn lift_parcel(sounding: &Sounding, z0: f64, t0: f64, td0: f64) -> ParcelResult {
    let lcl = z0 + lcl_height(t0, td0);
    let p0 = sounding.at(z0).p;
    let r0 = mixing_ratio(saturation_vapor_pressure(td0.min(t0)), p0);
    let mut t = t0;
    let mut z = z0;
    let mut cape = 0.0;
    let mut cin = 0.0;
    let mut cape3 = 0.0;
    let mut lfc = None;
    let mut el = None;
    let step = DZ / 2.0;
    let mut last_positive = false;
    while z < TOP {
        let env = sounding.at(z + step);
        // ascend
        if z + step <= lcl {
            t -= DRY_LAPSE * step;
        } else if z < lcl {
            let dry = lcl - z;
            t -= DRY_LAPSE * dry;
            t -= moist_lapse_rate(t, env.p) * (step - dry);
        } else {
            t -= moist_lapse_rate(t, env.p) * step;
        }
        z += step;
        let r_parcel = if z <= lcl { r0 } else { saturation_mixing_ratio(t, env.p) };
        let tv_p = virtual_temperature(t, r_parcel);
        let r_env = mixing_ratio(saturation_vapor_pressure(env.td), env.p);
        let tv_e = virtual_temperature(env.t, r_env);
        let b = G * (tv_p - tv_e) / tv_e;
        if b > 0.0 {
            if lfc.is_none() && z >= lcl - DZ {
                lfc = Some(z);
            }
            if lfc.is_some() {
                cape += b * step;
                if z <= 3000.0 {
                    cape3 += b * step;
                }
            }
            last_positive = true;
        } else {
            if lfc.is_none() {
                cin += b * step;
            } else if last_positive {
                el = Some(z);
                // allow a second positive layer only if it is shallow inhibition (< 25 J/kg)
            }
            last_positive = false;
        }
        if el.is_some() && !last_positive && z > el.unwrap() + 1500.0 {
            break;
        }
    }
    if lfc.is_none() {
        cin = 0.0f64.min(cin);
    }
    ParcelResult {
        cape,
        cin: cin.min(0.0),
        lcl: lcl - z0,
        lfc,
        el,
        cape_0_3km: cape3,
        w_max: (2.0 * cape).sqrt(),
    }
}

/// Bulk wind difference between two heights (m/s, and its components).
pub fn bulk_shear(s: &Sounding, z0: f64, z1: f64) -> (f64, f64, f64) {
    let (u0, v0) = s.wind(z0);
    let (u1, v1) = s.wind(z1);
    let (du, dv) = (u1 - u0, v1 - v0);
    ((du * du + dv * dv).sqrt(), du, dv)
}

/// Bunkers et al. (2000) supercell motion: right mover, left mover and the 0-6 km mean wind.
pub fn bunkers(s: &Sounding) -> ((f64, f64), (f64, f64), (f64, f64)) {
    let mean = s.mean_wind(0.0, 6000.0);
    let low = s.mean_wind(0.0, 500.0);
    let high = s.mean_wind(5500.0, 6000.0);
    let (su, sv) = (high.0 - low.0, high.1 - low.1);
    let mag = (su * su + sv * sv).sqrt().max(1e-6);
    let d = 7.5;
    // Rotate the shear vector 90 degrees clockwise for the right mover.
    let right = (mean.0 + d * sv / mag, mean.1 - d * su / mag);
    let left = (mean.0 - d * sv / mag, mean.1 + d * su / mag);
    (right, left, mean)
}

/// Storm-relative helicity (m^2/s^2) of the layer 0..`depth` for storm motion (cx, cy).
pub fn storm_relative_helicity(s: &Sounding, depth: f64, storm: (f64, f64)) -> f64 {
    let n = ((depth / DZ) as usize).max(1);
    let mut srh = 0.0;
    for k in 0..n {
        let (u0, v0) = s.wind(k as f64 * DZ);
        let (u1, v1) = s.wind((k + 1) as f64 * DZ);
        srh += (u1 - storm.0) * (v0 - storm.1) - (u0 - storm.0) * (v1 - storm.1);
    }
    srh
}

/// Downdraft CAPE (J/kg): the negative buoyancy of a saturated parcel from the level of
/// minimum theta-e in the lowest 400 hPa (as SPC computes it), brought down to the ground.
/// Large DCAPE favours strong cold pools, outflow and microbursts.
pub fn dcape(s: &Sounding) -> f64 {
    let top = s.height_of_pressure(s.at(0.0).p - 40_000.0);
    let mut start = 2000.0;
    let mut best = f64::MAX;
    let mut z = 500.0;
    while z <= top {
        let l = s.at(z);
        let te = super::thermo::theta_e(l.t, l.td, l.p);
        if te < best {
            best = te;
            start = z;
        }
        z += DZ;
    }
    // wet-bulb approximated by the one-third rule: Tw = T - (T - Td) / 3
    let l = s.at(start);
    let mut t = l.t - (l.t - l.td) / 3.0;
    let mut z = start;
    let mut dcape = 0.0;
    while z > 0.0 {
        let step = DZ.min(z);
        let env = s.at(z - step);
        t += moist_lapse_rate(t, env.p) * step;
        z -= step;
        let r_env = mixing_ratio(saturation_vapor_pressure(env.td), env.p);
        let tv_e = virtual_temperature(env.t, r_env);
        let tv_p = virtual_temperature(t, saturation_mixing_ratio(t, env.p));
        let b = G * (tv_p - tv_e) / tv_e;
        if b < 0.0 {
            dcape -= b * step;
        }
    }
    dcape
}

/// Environmental lapse rate of a layer (K/km).
pub fn lapse_rate(s: &Sounding, z0: f64, z1: f64) -> f64 {
    (s.at(z0).t - s.at(z1).t) / (z1 - z0) * 1000.0
}

/// Significant Tornado Parameter (Thompson et al. 2003/2012), fixed-layer form with a CIN term.
pub fn stp(sb_cape: f64, sb_lcl: f64, srh1: f64, shear6: f64, cin: f64) -> f64 {
    let cape_t = sb_cape / 1500.0;
    let lcl_t = if sb_lcl < 1000.0 {
        1.0
    } else if sb_lcl > 2000.0 {
        0.0
    } else {
        (2000.0 - sb_lcl) / 1000.0
    };
    let srh_t = srh1.max(0.0) / 150.0;
    let shear_t = if shear6 < 12.5 { 0.0 } else { (shear6 / 20.0).min(1.5) };
    let cin_t = if cin > -50.0 {
        1.0
    } else if cin < -200.0 {
        0.0
    } else {
        (200.0 + cin) / 150.0
    };
    (cape_t * lcl_t * srh_t * shear_t * cin_t).max(0.0)
}

/// Supercell Composite Parameter (Thompson et al. 2004).
pub fn scp(mu_cape: f64, srh3: f64, shear6: f64) -> f64 {
    let shear_t = if shear6 < 10.0 { 0.0 } else { (shear6 / 20.0).min(1.0) };
    ((mu_cape / 1000.0) * (srh3.max(0.0) / 50.0) * shear_t).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atmosphere::presets::Preset;
    use crate::units::T0;

    #[test]
    fn classic_supercell_environment_has_large_cape_and_shear() {
        let s = Preset::Classic.spec().build();
        let sfc = s.at(0.0);
        let p = lift_parcel(&s, 0.0, sfc.t, sfc.td);
        assert!(p.cape > 1500.0 && p.cape < 5000.0, "cape {}", p.cape);
        assert!(p.lfc.is_some() && p.el.is_some());
        assert!(p.el.unwrap() > 9000.0, "el {:?}", p.el);
        let (sh6, _, _) = bulk_shear(&s, 0.0, 6000.0);
        assert!(sh6 > 20.0, "{sh6}");
    }

    #[test]
    fn stable_profile_has_no_cape() {
        let mut spec = Preset::Classic.spec();
        spec.surface_temp_c = 8.0;
        spec.surface_dewpoint_c = -5.0;
        spec.mid_lapse = 5.0;
        let s = spec.build();
        let sfc = s.at(0.0);
        let p = lift_parcel(&s, 0.0, sfc.t, sfc.td);
        assert!(p.cape < 50.0, "{}", p.cape);
    }

    #[test]
    fn veering_hodograph_has_positive_srh_for_the_right_mover() {
        let s = Preset::Outbreak.spec().build();
        let (rm, _, _) = bunkers(&s);
        let srh1 = storm_relative_helicity(&s, 1000.0, rm);
        let srh3 = storm_relative_helicity(&s, 3000.0, rm);
        assert!(srh1 > 150.0, "{srh1}");
        assert!(srh3 >= srh1 * 0.9, "{srh3}");
    }

    #[test]
    fn straight_hodograph_has_little_helicity_relative_to_mean_wind() {
        let mut spec = Preset::Classic.spec();
        spec.hodograph = vec![(0.0, 5.0, 270.0), (6000.0, 30.0, 270.0), (12000.0, 40.0, 270.0)];
        let s = spec.build();
        let mean = s.mean_wind(0.0, 6000.0);
        assert!(storm_relative_helicity(&s, 3000.0, mean).abs() < 5.0);
    }

    #[test]
    fn stp_terms() {
        assert_eq!(stp(3000.0, 2500.0, 300.0, 25.0, 0.0), 0.0); // high LCL
        assert_eq!(stp(3000.0, 800.0, 300.0, 10.0, 0.0), 0.0); // weak shear
        let v = stp(1500.0, 800.0, 150.0, 20.0, 0.0);
        assert!((v - 1.0).abs() < 1e-9);
    }

    #[test]
    fn convective_profiles_have_plausible_dcape() {
        for p in [Preset::Classic, Preset::DryMicroburst, Preset::Outbreak] {
            let d = dcape(&p.spec().build());
            assert!((600.0..2000.0).contains(&d), "{p:?} {d}");
        }
        let moist = dcape(&Preset::HighPrecip.spec().build());
        assert!(moist < dcape(&Preset::Classic.spec().build()));
        let _ = T0;
    }
}
