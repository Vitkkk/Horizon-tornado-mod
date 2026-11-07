// The simplified atmosphere: a sounding and the convective parameters derived from it,
// computed once when the environment changes and cached (they drive storm formation, storm
// mode, mesocyclone strength and tornadogenesis).

pub mod indices;
pub mod presets;
pub mod sounding;
pub mod thermo;

use crate::math::Vec3;
use indices::{ParcelResult, bulk_shear, bunkers, dcape, lapse_rate, lift_parcel, scp, stp, storm_relative_helicity};
use presets::Preset;
use sounding::{EnvironmentSpec, Sounding};

/// A sounding with every parameter the storms use.
#[derive(Clone, Debug)]
pub struct Environment {
    pub preset: Option<Preset>,
    pub spec: EnvironmentSpec,
    pub sounding: Sounding,
    /// Surface-based and mixed-layer (lowest 1 km mean) parcels.
    pub sb: ParcelResult,
    pub ml: ParcelResult,
    /// Bulk shear magnitudes (m/s).
    pub shear_0_1: f64,
    pub shear_0_3: f64,
    pub shear_0_6: f64,
    /// Bunkers right- and left-mover motion and the 0-6 km mean wind, (u, v) m/s.
    pub right_mover: (f64, f64),
    pub left_mover: (f64, f64),
    pub mean_wind: (f64, f64),
    /// Storm-relative helicity for the right mover (m^2/s^2).
    pub srh_0_1: f64,
    pub srh_0_3: f64,
    /// Storm-relative wind at anvil level (9-11 km) for the right mover (m/s).
    pub anvil_sr_wind: f64,
    pub stp: f64,
    pub scp: f64,
    pub dcape: f64,
    /// Precipitable water (mm).
    pub pw: f64,
    pub lapse_0_3: f64,
    pub lapse_3_6: f64,
    /// Mean relative humidity 0-3 km and 3-6 km (0..1).
    pub rh_low: f64,
    pub rh_mid: f64,
}

impl Environment {
    pub fn from_preset(p: Preset) -> Self {
        let mut e = Self::from_spec(p.spec());
        e.preset = Some(p);
        e
    }

    pub fn from_spec(spec: EnvironmentSpec) -> Self {
        let sounding = spec.build();
        let sfc = sounding.at(0.0);
        let sb = lift_parcel(&sounding, 0.0, sfc.t, sfc.td);
        // mixed layer: mean potential temperature and mixing ratio of the lowest 1 km,
        // brought to the surface pressure
        let ml = {
            let (mut th, mut r, mut n) = (0.0, 0.0, 0.0);
            let mut z = 0.0;
            while z <= 1000.0 {
                let l = sounding.at(z);
                th += thermo::potential_temperature(l.t, l.p);
                r += thermo::mixing_ratio(thermo::saturation_vapor_pressure(l.td), l.p);
                n += 1.0;
                z += 100.0;
            }
            let (th, r) = (th / n, r / n);
            let t = th * (sfc.p / 100_000.0).powf(crate::units::KAPPA);
            let e = r * sfc.p / (crate::units::EPSILON + r);
            let td = thermo::dewpoint_from_vapor_pressure(e).min(t);
            lift_parcel(&sounding, 0.0, t, td)
        };
        let (rm, lm, mean) = bunkers(&sounding);
        let srh_0_1 = storm_relative_helicity(&sounding, 1000.0, rm);
        let srh_0_3 = storm_relative_helicity(&sounding, 3000.0, rm);
        let shear_0_1 = bulk_shear(&sounding, 0.0, 1000.0).0;
        let shear_0_3 = bulk_shear(&sounding, 0.0, 3000.0).0;
        let shear_0_6 = bulk_shear(&sounding, 0.0, 6000.0).0;
        let anvil = sounding.mean_wind(9000.0, 11000.0);
        let anvil_sr_wind = ((anvil.0 - rm.0).powi(2) + (anvil.1 - rm.1).powi(2)).sqrt();
        let mu_cape = sb.cape.max(ml.cape);
        Environment {
            preset: None,
            stp: stp(sb.cape, sb.lcl, srh_0_1, shear_0_6, sb.cin),
            scp: scp(mu_cape, srh_0_3, shear_0_6),
            dcape: dcape(&sounding),
            pw: sounding.precipitable_water(),
            lapse_0_3: lapse_rate(&sounding, 0.0, 3000.0),
            lapse_3_6: lapse_rate(&sounding, 3000.0, 6000.0),
            rh_low: sounding.mean_rh(0.0, 3000.0),
            rh_mid: sounding.mean_rh(3000.0, 6000.0),
            spec,
            sounding,
            sb,
            ml,
            shear_0_1,
            shear_0_3,
            shear_0_6,
            right_mover: rm,
            left_mover: lm,
            mean_wind: mean,
            srh_0_1,
            srh_0_3,
            anvil_sr_wind,
        }
    }

    /// The environmental wind at height `z` (m AGL) as a world vector (m/s).
    pub fn wind(&self, z: f64) -> Vec3 {
        let (u, v) = self.sounding.wind(z.max(0.0));
        Vec3::from_met(u, v, 0.0)
    }

    /// The right-mover storm motion as a world vector.
    pub fn storm_motion(&self) -> Vec3 {
        Vec3::from_met(self.right_mover.0, self.right_mover.1, 0.0)
    }

    /// Air density near height `z` (kg/m^3).
    pub fn density(&self, z: f64) -> f64 {
        let l = self.sounding.at(z.max(0.0));
        l.p / (crate::units::RD * l.t)
    }

    /// Precipitation efficiency proxy (0..1): moist columns rain out more of their condensate.
    pub fn precip_efficiency(&self) -> f64 {
        crate::math::clamp01(0.25 + (self.pw - 20.0) / 50.0 + (self.rh_mid - 0.5) * 0.6)
    }

    /// Microburst potential (0 ~ none, > 1 likely): a dry term for inverted-V profiles (DCAPE
    /// with a nearly dry-adiabatic, dry sub-cloud layer) and a wet term for moist, unstable,
    /// weakly sheared air whose cores load heavily with precipitation and hail.
    pub fn microburst_index(&self) -> f64 {
        let sfc = self.sounding.at(0.0);
        let depression = sfc.t - sfc.td;
        let dry = (self.dcape / 1000.0)
            * crate::math::smoothstep(7.5, 9.5, self.lapse_0_3)
            * crate::math::smoothstep(10.0, 25.0, depression);
        let wet = (self.sb.cape / 2500.0)
            * crate::math::smoothstep(30.0, 45.0, self.pw)
            * (1.0 - crate::math::smoothstep(10.0, 20.0, self.shear_0_6))
            * (self.dcape / 1000.0);
        dry + wet
    }

    /// One-line summary for /storm debug.
    pub fn summary_lines(&self) -> Vec<String> {
        let f = |o: Option<f64>| o.map_or("-".to_string(), |v| format!("{v:.0} m"));
        vec![
            format!(
                "Ambiente {}: SBCAPE {:.0} J/kg, SBCIN {:.0} J/kg, MLCAPE {:.0}, 0-3km CAPE {:.0}",
                self.preset.map_or("custom", Preset::name),
                self.sb.cape,
                self.sb.cin,
                self.ml.cape,
                self.sb.cape_0_3km
            ),
            format!(
                "LCL {:.0} m, LFC {}, EL {}, DCAPE {:.0}, PW {:.0} mm, lapse 0-3 {:.1} / 3-6 {:.1} K/km",
                self.sb.lcl,
                f(self.sb.lfc),
                f(self.sb.el),
                self.dcape,
                self.pw,
                self.lapse_0_3,
                self.lapse_3_6
            ),
            format!(
                "Shear 0-1 {:.0} / 0-3 {:.0} / 0-6 {:.0} m/s, SRH 0-1 {:.0} / 0-3 {:.0} m2/s2",
                self.shear_0_1, self.shear_0_3, self.shear_0_6, self.srh_0_1, self.srh_0_3
            ),
            format!(
                "Bunkers RM ({:.1}, {:.1}) m/s, SR anvil {:.0} m/s, STP {:.1}, SCP {:.1}",
                self.right_mover.0, self.right_mover.1, self.anvil_sr_wind, self.stp, self.scp
            ),
        ]
    }
}

impl Default for Environment {
    fn default() -> Self {
        Environment::from_preset(Preset::Classic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbreak_is_more_tornadic_than_classic_and_marginal() {
        let o = Environment::from_preset(Preset::Outbreak);
        let c = Environment::from_preset(Preset::Classic);
        let m = Environment::from_preset(Preset::Marginal);
        assert!(o.stp > c.stp, "{} {}", o.stp, c.stp);
        assert!(c.stp > m.stp, "{} {}", c.stp, m.stp);
        assert!(o.stp > 2.0, "{}", o.stp);
    }

    #[test]
    fn lp_has_higher_lcl_and_stronger_anvil_winds_than_hp() {
        let lp = Environment::from_preset(Preset::LowPrecip);
        let hp = Environment::from_preset(Preset::HighPrecip);
        assert!(lp.sb.lcl > hp.sb.lcl + 800.0, "{} {}", lp.sb.lcl, hp.sb.lcl);
        assert!(lp.anvil_sr_wind > hp.anvil_sr_wind);
        assert!(hp.precip_efficiency() > lp.precip_efficiency());
    }

    #[test]
    fn inverted_v_profile_favours_microbursts() {
        let dry = Environment::from_preset(Preset::DryMicroburst).microburst_index();
        let classic = Environment::from_preset(Preset::Classic).microburst_index();
        let qlcs = Environment::from_preset(Preset::Qlcs).microburst_index();
        assert!(dry > 0.8, "{dry}");
        assert!(dry > 2.0 * classic.max(qlcs), "{dry} {classic} {qlcs}");
    }

    #[test]
    fn pulse_environment_has_weak_shear() {
        let p = Environment::from_preset(Preset::Pulse);
        assert!(p.shear_0_6 < 10.0);
        assert!(p.sb.cape > 1000.0, "{}", p.sb.cape);
    }
}
