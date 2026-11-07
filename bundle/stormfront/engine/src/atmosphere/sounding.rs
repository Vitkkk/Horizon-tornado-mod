// A vertical profile of the environment (a "sounding"): temperature, dewpoint, pressure and
// wind every 100 m from the ground to 16 km, built from a few physically meaningful
// parameters (surface temperature and dewpoint, lapse rates, moist-layer depth, hodograph).

use super::thermo::{
    dewpoint_from_vapor_pressure, mixing_ratio, saturation_vapor_pressure, virtual_temperature,
};
use crate::units::{G, RD, T0};

/// Spacing of the levels (m).
pub const DZ: f64 = 100.0;
/// Top of the profile (m above ground).
pub const TOP: f64 = 16_000.0;

/// What a sounding is built from. Temperatures in degC, heights in m, winds in m/s.
#[derive(Clone, Debug, PartialEq)]
pub struct EnvironmentSpec {
    pub surface_temp_c: f64,
    pub surface_dewpoint_c: f64,
    pub surface_pressure_pa: f64,
    /// Lapse rate of the boundary layer, to `mixed_layer_depth` (K/km).
    pub boundary_lapse: f64,
    /// Depth of the well-mixed boundary layer (m).
    pub mixed_layer_depth: f64,
    /// Strength of a capping inversion at the top of the boundary layer (K warmer over 300 m).
    pub cap_strength: f64,
    /// Lapse rate of the 700-500 hPa layer, about 3-6 km (K/km).
    pub mid_lapse: f64,
    /// Depth of the moist layer above the ground (m); dewpoints fall off above it.
    pub moist_depth: f64,
    /// Dewpoint depression of the free troposphere above the moist layer (K).
    pub dry_aloft_depression: f64,
    /// Tropopause height (m).
    pub tropopause: f64,
    /// Hodograph points: (height m, speed m/s, direction the wind comes FROM, degrees).
    pub hodograph: Vec<(f64, f64, f64)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Level {
    /// Height above ground (m).
    pub z: f64,
    pub p: f64,
    pub t: f64,
    pub td: f64,
    /// Wind towards east and north (m/s).
    pub u: f64,
    pub v: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sounding {
    pub levels: Vec<Level>,
}

/// (u east, v north) of a wind of `speed` coming from `from_deg` (meteorological convention).
pub fn wind_components(speed: f64, from_deg: f64) -> (f64, f64) {
    let a = from_deg.to_radians();
    (-speed * a.sin(), -speed * a.cos())
}

impl EnvironmentSpec {
    pub fn build(&self) -> Sounding {
        let n = (TOP / DZ) as usize + 1;
        let mut levels = Vec::with_capacity(n);
        let t_sfc = self.surface_temp_c + T0;
        let td_sfc = self.surface_dewpoint_c.min(self.surface_temp_c) + T0;
        let mut p = self.surface_pressure_pa;
        let mut t = t_sfc;
        // Surface mixing ratio, kept nearly constant through the moist layer.
        let r_sfc = mixing_ratio(saturation_vapor_pressure(td_sfc), p);
        let mut prev_tv = virtual_temperature(t, r_sfc);
        for i in 0..n {
            let z = i as f64 * DZ;
            if i > 0 {
                // Temperature: boundary layer, cap, low-mid levels, mid levels, then isothermal
                // stratosphere.
                let lapse = if z <= self.mixed_layer_depth {
                    self.boundary_lapse / 1000.0
                } else if z <= self.mixed_layer_depth + 300.0 {
                    -self.cap_strength / 300.0
                } else if z <= 3000.0 {
                    // blend from the boundary layer lapse to the mid-level one
                    let f = ((z - self.mixed_layer_depth) / (3000.0 - self.mixed_layer_depth).max(1.0))
                        .clamp(0.0, 1.0);
                    (self.boundary_lapse * (1.0 - f) * 0.85 + self.mid_lapse * f) / 1000.0
                } else if z <= 6500.0 {
                    self.mid_lapse / 1000.0
                } else if z <= self.tropopause {
                    7.0 / 1000.0
                } else {
                    0.0
                };
                t -= lapse * DZ;
            }
            // Moisture: mixing ratio slowly decreasing in the moist layer, then a dry free
            // troposphere with a fixed dewpoint depression.
            let e_moist = {
                let frac = (1.0 - 0.25 * (z / self.moist_depth.max(1.0)).min(1.0)).max(0.05);
                let r = r_sfc * frac;
                r * p / (crate::units::EPSILON + r)
            };
            let td_moist = dewpoint_from_vapor_pressure(e_moist);
            let td_dry = t - self.dry_aloft_depression;
            let blend = ((z - self.moist_depth) / 600.0).clamp(0.0, 1.0);
            let mut td = td_moist * (1.0 - blend) + td_dry.min(td_moist) * blend;
            if z > self.tropopause {
                td = td.min(t - 25.0);
            }
            let td = td.min(t);
            if i > 0 {
                let r = mixing_ratio(saturation_vapor_pressure(td), p);
                let tv = virtual_temperature(t, r);
                let tv_mean = 0.5 * (tv + prev_tv);
                p *= (-G * DZ / (RD * tv_mean)).exp();
                prev_tv = tv;
            }
            let (u, v) = self.wind_at(z);
            levels.push(Level { z, p, t, td, u, v });
        }
        Sounding { levels }
    }

    /// The wind at height `z`, interpolating the hodograph in (u, v).
    pub fn wind_at(&self, z: f64) -> (f64, f64) {
        let pts: Vec<(f64, f64, f64)> = self
            .hodograph
            .iter()
            .map(|&(h, s, d)| {
                let (u, v) = wind_components(s, d);
                (h, u, v)
            })
            .collect();
        match pts.as_slice() {
            [] => (0.0, 0.0),
            [only] => (only.1, only.2),
            _ => {
                if z <= pts[0].0 {
                    return (pts[0].1, pts[0].2);
                }
                for w in pts.windows(2) {
                    if z <= w[1].0 {
                        let f = (z - w[0].0) / (w[1].0 - w[0].0);
                        return (w[0].1 + (w[1].1 - w[0].1) * f, w[0].2 + (w[1].2 - w[0].2) * f);
                    }
                }
                let last = pts[pts.len() - 1];
                (last.1, last.2)
            }
        }
    }
}

impl Sounding {
    /// The level nearest below `z` and the fraction to the next.
    fn locate(&self, z: f64) -> (usize, f64) {
        let zc = z.clamp(0.0, TOP);
        let i = ((zc / DZ) as usize).min(self.levels.len().saturating_sub(2));
        (i, (zc - i as f64 * DZ) / DZ)
    }

    /// Interpolated level at height `z` (m above ground).
    pub fn at(&self, z: f64) -> Level {
        let (i, f) = self.locate(z);
        let a = self.levels[i];
        let b = self.levels[(i + 1).min(self.levels.len() - 1)];
        let mix = |x: f64, y: f64| x + (y - x) * f;
        Level {
            z,
            p: a.p * (b.p / a.p).powf(f),
            t: mix(a.t, b.t),
            td: mix(a.td, b.td),
            u: mix(a.u, b.u),
            v: mix(a.v, b.v),
        }
    }

    pub fn wind(&self, z: f64) -> (f64, f64) {
        let l = self.at(z);
        (l.u, l.v)
    }

    /// Mean wind of a layer (simple height-weighted average, m/s).
    pub fn mean_wind(&self, z0: f64, z1: f64) -> (f64, f64) {
        let n = (((z1 - z0) / DZ).ceil() as usize).max(1);
        let (mut su, mut sv) = (0.0, 0.0);
        for k in 0..=n {
            let (u, v) = self.wind(z0 + (z1 - z0) * k as f64 / n as f64);
            su += u;
            sv += v;
        }
        (su / (n + 1) as f64, sv / (n + 1) as f64)
    }

    /// Height (m) at which the environment reaches pressure `p`.
    pub fn height_of_pressure(&self, p: f64) -> f64 {
        for w in self.levels.windows(2) {
            if w[1].p <= p {
                let f = (w[0].p / p).ln() / (w[0].p / w[1].p).ln();
                return w[0].z + f * DZ;
            }
        }
        TOP
    }

    /// Precipitable water (mm): the column's water vapour.
    pub fn precipitable_water(&self) -> f64 {
        let mut pw = 0.0;
        for w in self.levels.windows(2) {
            let r0 = mixing_ratio(saturation_vapor_pressure(w[0].td), w[0].p);
            let r1 = mixing_ratio(saturation_vapor_pressure(w[1].td), w[1].p);
            pw += 0.5 * (r0 + r1) * (w[0].p - w[1].p) / G;
        }
        pw // kg/m^2 = mm
    }

    /// Mean relative humidity of a layer.
    pub fn mean_rh(&self, z0: f64, z1: f64) -> f64 {
        let n = (((z1 - z0) / DZ).ceil() as usize).max(1);
        (0..=n)
            .map(|k| {
                let l = self.at(z0 + (z1 - z0) * k as f64 / n as f64);
                super::thermo::relative_humidity(l.t, l.td)
            })
            .sum::<f64>()
            / (n + 1) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atmosphere::presets::Preset;

    #[test]
    fn pressure_falls_with_height_like_the_standard_atmosphere() {
        let s = Preset::Classic.spec().build();
        let p5 = s.at(5500.0).p;
        assert!((48_000.0..54_000.0).contains(&p5), "500 hPa near 5.5 km, got {p5}");
        assert!(s.levels.windows(2).all(|w| w[1].p < w[0].p));
    }

    #[test]
    fn dewpoint_never_exceeds_temperature() {
        for p in Preset::ALL {
            let s = p.spec().build();
            assert!(s.levels.iter().all(|l| l.td <= l.t + 1e-9), "{p:?}");
        }
    }

    #[test]
    fn wind_from_south_blows_north() {
        let (u, v) = wind_components(10.0, 180.0);
        assert!(u.abs() < 1e-9 && (v - 10.0).abs() < 1e-9);
        let (u, v) = wind_components(10.0, 270.0);
        assert!((u - 10.0).abs() < 1e-9 && v.abs() < 1e-9);
    }

    #[test]
    fn precipitable_water_is_plausible() {
        let hp = Preset::HighPrecip.spec().build().precipitable_water();
        let lp = Preset::LowPrecip.spec().build().precipitable_water();
        assert!(hp > lp, "{hp} {lp}");
        assert!((20.0..70.0).contains(&hp), "{hp}");
    }
}
