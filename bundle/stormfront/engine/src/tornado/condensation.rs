// The visible condensation funnel and the tornado's appearance.
//
// Air entering the vortex loses pressure; it condenses where the pressure deficit lowers its
// saturation level below its height. Hydrostatically, lowering the cloud base from the LCL to
// height z takes a deficit of about rho g (z_LCL - z) (roughly 12 hPa per 100 m), so the funnel
// at each height is the radius where the cyclostrophic deficit equals that. The funnel's width
// therefore differs from the damaging-wind width, and a weak vortex under a high cloud base
// shows only a funnel aloft and a debris whirl at the ground.
//
// The display maps heights linearly: world height y = f * cloud_base (blocks above the ground)
// stands for physical height f * LCL.

use super::multivortex::MultiVortex;
use super::vortex::VortexParams;
use crate::math::Vec3;
use crate::units::G;

/// Levels sampled from the ground (index 0) to the cloud base.
pub const FUNNEL_LEVELS: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Morphology {
    /// Condensation only aloft; a dust/debris whirl below.
    #[default]
    FunnelAloft,
    Rope,
    Cone,
    Stovepipe,
    Wedge,
    ElephantTrunk,
    MultipleVortex,
    RainWrapped,
}

impl Morphology {
    pub fn name(self) -> &'static str {
        match self {
            Morphology::FunnelAloft => "funnel aloft",
            Morphology::Rope => "rope",
            Morphology::Cone => "cone",
            Morphology::Stovepipe => "stovepipe",
            Morphology::Wedge => "wedge",
            Morphology::ElephantTrunk => "elephant trunk",
            Morphology::MultipleVortex => "multiple-vortex",
            Morphology::RainWrapped => "rain-wrapped",
        }
    }
    pub fn id(self) -> u8 {
        self as u8
    }
    pub fn from_id(id: u8) -> Morphology {
        match id {
            1 => Morphology::Rope,
            2 => Morphology::Cone,
            3 => Morphology::Stovepipe,
            4 => Morphology::Wedge,
            5 => Morphology::ElephantTrunk,
            6 => Morphology::MultipleVortex,
            7 => Morphology::RainWrapped,
            _ => Morphology::FunnelAloft,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FunnelLevel {
    /// Height above the ground (blocks).
    pub height: f64,
    /// Condensation radius (blocks); 0 where the air does not condense.
    pub radius: f64,
    /// Axis displacement from the ground centre (horizontal, blocks).
    pub offset: Vec3,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Funnel {
    pub levels: Vec<FunnelLevel>,
    /// The lowest condensed height (blocks above ground); 0 when the funnel touches down.
    pub bottom: f64,
    pub morphology: Morphology,
}

/// What shapes the funnel besides the vortex itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FunnelInputs {
    /// Physical LCL of the inflow air (m).
    pub lcl: f64,
    /// Display height of the cloud base above the ground (blocks).
    pub cloud_base: f64,
    /// Axis displacement at the cloud base (blocks).
    pub tilt: Vec3,
    /// Rope-like waviness (0..1) and its phase.
    pub waviness: f64,
    pub wave_phase: f64,
    /// Air density near the ground.
    pub rho: f64,
    /// Fraction of the funnel hidden in rain (0..1).
    pub rain_wrap: f64,
}

/// The vortex at height fraction `f` (0 ground, 1 cloud base): it widens towards the cloud
/// base keeping its circulation (v_max * r_max), the flare that makes cones and wall clouds.
pub fn vortex_at_level(p: &VortexParams, f: f64) -> VortexParams {
    let mut q = *p;
    let flare = 1.0 + 2.2 * f.powi(3);
    q.r_max = p.r_max * flare;
    q.v_max = p.v_max / flare;
    q
}

/// The air a tornado ingests near the ground is moister than the surface-based parcel the
/// environmental LCL is computed from (moisture converges in the inflow, and rain-cooled
/// outflow air nearly saturated is drawn in from the rear flank), so its own condensation
/// level is lower. This factor scales the environmental LCL for the funnel.
pub const INFLOW_LCL_FACTOR: f64 = 0.5;

pub fn compute_funnel(p: &VortexParams, mv: &MultiVortex, inp: &FunnelInputs) -> Funnel {
    let mut levels = Vec::with_capacity(FUNNEL_LEVELS);
    let lcl = (inp.lcl * INFLOW_LCL_FACTOR).max(30.0);
    let side = Vec3::new(-inp.tilt.z, 0.0, inp.tilt.x).normalized();
    let mut bottom = inp.cloud_base;
    for k in 0..FUNNEL_LEVELS {
        let f = k as f64 / (FUNNEL_LEVELS - 1) as f64;
        let height = f * inp.cloud_base;
        let q = vortex_at_level(p, f);
        // the ground level is drawn from the air a couple of blocks up
        let sf = q.surface_factor(height.max(2.0));
        let need = inp.rho * G * lcl * (1.0 - f) / (sf * sf).max(1e-3);
        let radius = if f >= 0.999 {
            // at the cloud base everything is cloud; draw the base as wide as the flare
            q.r_max * 1.2
        } else {
            q.radius_of_deficit(need, inp.rho).map_or(0.0, |r| r.min(q.r_max * 1.5))
        };
        let wave = inp.waviness
            * (f * std::f64::consts::TAU * 1.5 + inp.wave_phase).sin()
            * 4.0
            * f
            * (1.0 - f)
            * (0.15 * inp.cloud_base);
        let offset = inp.tilt * f.powf(1.3) + side * wave;
        if radius > 0.05 && height < bottom {
            bottom = height;
        }
        levels.push(FunnelLevel { height, radius, offset });
    }
    // keep the condensation continuous from the cloud base downward
    let mut gap = false;
    for k in (0..FUNNEL_LEVELS).rev() {
        if levels[k].radius <= 0.05 {
            gap = true;
        }
        if gap {
            levels[k].radius = 0.0;
        }
    }
    bottom = levels.iter().find(|l| l.radius > 0.05).map_or(inp.cloud_base, |l| l.height);
    let morphology = classify(&levels, bottom, p, mv, inp);
    Funnel { levels, bottom, morphology }
}

fn classify(levels: &[FunnelLevel], bottom: f64, p: &VortexParams, mv: &MultiVortex, inp: &FunnelInputs) -> Morphology {
    let h = inp.cloud_base.max(1.0);
    if inp.rain_wrap > 0.7 && p.v_max > 30.0 {
        return Morphology::RainWrapped;
    }
    let active_subs = mv.enabled && mv.vortices.iter().filter(|v| v.envelope() > 0.3).count() >= 2;
    if active_subs && (bottom > 0.25 * h || p.beta > 1.2) {
        return Morphology::MultipleVortex;
    }
    if bottom > 0.45 * h {
        return Morphology::FunnelAloft;
    }
    let at = |f: f64| -> f64 {
        let k = ((f * (FUNNEL_LEVELS - 1) as f64).round() as usize).min(FUNNEL_LEVELS - 1);
        levels[k].radius
    };
    let low = (at(0.1) + at(0.2) + at(0.3)) / 3.0;
    let width_ratio = 2.0 * low / h;
    let taper = at(0.8) / low.max(0.1);
    let tilt = inp.tilt.horizontal_length() / h;
    if width_ratio >= 0.85 {
        Morphology::Wedge
    } else if width_ratio < 0.18 && (tilt > 0.3 || inp.waviness > 0.4) {
        Morphology::Rope
    } else if tilt > 0.3 && width_ratio < 0.45 {
        Morphology::ElephantTrunk
    } else if taper < 1.7 && width_ratio >= 0.2 {
        Morphology::Stovepipe
    } else {
        Morphology::Cone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(lcl: f64) -> FunnelInputs {
        FunnelInputs {
            lcl,
            cloud_base: 150.0,
            tilt: Vec3::ZERO,
            waviness: 0.0,
            wave_phase: 0.0,
            rho: 1.15,
            rain_wrap: 0.0,
        }
    }

    #[test]
    fn strong_vortex_under_low_base_touches_down() {
        let p = VortexParams::new(85.0, 200.0);
        let f = compute_funnel(&p, &MultiVortex::default(), &inputs(500.0));
        assert_eq!(f.bottom, 0.0, "{f:?}");
        assert_eq!(f.morphology, Morphology::Wedge);
    }

    #[test]
    fn weak_vortex_under_high_base_condenses_only_aloft() {
        let p = VortexParams::new(35.0, 40.0);
        let f = compute_funnel(&p, &MultiVortex::default(), &inputs(1500.0));
        assert!(f.bottom > 60.0, "{}", f.bottom);
        assert_eq!(f.morphology, Morphology::FunnelAloft);
    }

    #[test]
    fn condensation_is_narrower_than_damaging_winds_for_a_cone() {
        let p = VortexParams::new(70.0, 60.0);
        let f = compute_funnel(&p, &MultiVortex::default(), &inputs(700.0));
        let ground_funnel = f.levels[1].radius;
        let damaging = p.radius_of_wind(40.0, 0.0);
        assert!(ground_funnel > 0.0);
        assert!(damaging > ground_funnel * 1.5, "{damaging} vs {ground_funnel}");
        // a cone or stovepipe, widening upward
        assert!(f.levels[18].radius > f.levels[2].radius);
    }

    #[test]
    fn thin_tilted_vortex_is_a_rope() {
        let p = VortexParams::new(50.0, 12.0);
        let mut i = inputs(500.0);
        i.tilt = Vec3::new(80.0, 0.0, 0.0);
        i.waviness = 0.7;
        let f = compute_funnel(&p, &MultiVortex::default(), &i);
        assert_eq!(f.morphology, Morphology::Rope, "{f:?}");
    }
}
