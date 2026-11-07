// Tornadoes: the vortex wind field, multiple vortices, the condensation funnel and the
// life cycle.

pub mod condensation;
pub mod dynamics;
pub mod multivortex;
pub mod vortex;

pub use condensation::{Funnel, Morphology};
pub use dynamics::{SpawnShape, StormForcing, Tornado, TornadoStage};
pub use vortex::{RadialProfile, VortexParams};

/// A simulation-only wind-speed class of a tornado's peak wind (m/s), separate from any
/// damage-based rating: T0 < 40, T1 < 50, T2 < 61, T3 < 74, T4 < 89, T5 >= 89 (thresholds of
/// the EF scale's three-second gusts, converted to m/s). The survey rates damage on its own.
pub fn wind_class(v: f64) -> u8 {
    match v {
        v if v < 40.0 => 0,
        v if v < 50.0 => 1,
        v if v < 61.0 => 2,
        v if v < 74.0 => 3,
        v if v < 89.0 => 4,
        _ => 5,
    }
}
