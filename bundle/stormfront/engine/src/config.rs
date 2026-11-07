// Settings shared by the server simulation and the client. Physics settings live on the
// server; visual settings (quality, particle budgets) only on the client, so a phone running
// "low" sees fewer particles while the damage the server computes stays the same.

/// Visual quality of the client. It never changes the server's damage physics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Quality {
    Low,
    #[default]
    Medium,
    High,
}

impl Quality {
    pub fn parse(s: &str) -> Option<Quality> {
        match s.to_ascii_lowercase().as_str() {
            "low" | "baixa" | "l" => Some(Quality::Low),
            "medium" | "media" | "média" | "m" => Some(Quality::Medium),
            "high" | "alta" | "h" => Some(Quality::High),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Quality::Low => "low",
            Quality::Medium => "medium",
            Quality::High => "high",
        }
    }
    /// The most live particles the client keeps.
    pub fn particle_budget(self) -> usize {
        match self {
            Quality::Low => 2_500,
            Quality::Medium => 9_000,
            Quality::High => 24_000,
        }
    }
    /// Multiplier of emission rates (before the budget caps them).
    pub fn emission_scale(self) -> f64 {
        match self {
            Quality::Low => 0.3,
            Quality::Medium => 0.65,
            Quality::High => 1.0,
        }
    }
    /// Radar sweep: radials and gates.
    pub fn radar_resolution(self) -> (usize, usize) {
        match self {
            Quality::Low => (180, 120),
            Quality::Medium => (360, 180),
            Quality::High => (360, 240),
        }
    }
    /// Sub-samples per radar gate across the beam (beam filling).
    pub fn radar_subsamples(self) -> usize {
        match self {
            Quality::Low => 2,
            Quality::Medium => 3,
            Quality::High => 5,
        }
    }
}

/// How storm-scale meteorology is fitted into a Minecraft world. Tornado winds and the damage
/// they do stay at 1 block = 1 m; storm-scale geometry and slow storm evolution are compressed
/// so a whole supercell fits in a few render distances and lives for tens of minutes, not hours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldScale {
    /// Storm-scale horizontal lengths (cores, mesocyclones, outflow) are multiplied by this.
    pub storm_length: f64,
    /// Heights of clouds above the ground (LCL, cloud tops) are multiplied by this for display
    /// and for where the funnel attaches; the physics of condensation is done at real heights.
    pub cloud_height: f64,
    /// The highest a cloud base is drawn above the ground (blocks).
    pub max_cloud_base: f64,
    /// Storm lifecycle runs this much faster than real time (tornado winds are real time).
    pub storm_time: f64,
}

impl Default for WorldScale {
    fn default() -> Self {
        Self { storm_length: 0.25, cloud_height: 0.15, max_cloud_base: 190.0, storm_time: 4.0 }
    }
}

/// Server-side physics settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ServerSettings {
    pub scale: WorldScale,
    /// Whether tornadoes damage the world (`/tornado damage on|off`).
    pub damage: bool,
    /// Columns scanned around a tornado per damage pass (radius beyond the damaging winds).
    pub damage_margin: f64,
    /// Most physical debris pieces simulated at once.
    pub max_debris: usize,
    /// Most block edits flushed per batch (one batch resends the touched chunks).
    pub max_edits_per_flush: usize,
    /// Ticks between edit flushes.
    pub flush_interval_ticks: u32,
    /// Ticks between state messages to players.
    pub sync_interval_ticks: u32,
    /// Ticks between debris messages to players.
    pub debris_sync_interval_ticks: u32,
    /// Whether storms form on their own from the environment.
    pub natural_storms: bool,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            scale: WorldScale::default(),
            damage: true,
            damage_margin: 12.0,
            max_debris: 600,
            max_edits_per_flush: 6_000,
            flush_interval_ticks: 4,
            sync_interval_ticks: 5,
            debris_sync_interval_ticks: 2,
            natural_storms: false,
        }
    }
}

/// Client-side settings.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct ClientSettings {
    pub quality: Quality,
    /// The scientific overlay (wind vectors, damage-swath bounds, subvortex tracks...).
    pub science: bool,
    /// Camera shake near a tornado.
    pub shake: bool,
    /// Sounds.
    pub sound: bool,
}

impl ClientSettings {
    pub fn new() -> Self {
        Self { quality: Quality::Medium, science: false, shake: true, sound: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_budgets_grow() {
        assert!(Quality::Low.particle_budget() < Quality::Medium.particle_budget());
        assert!(Quality::Medium.particle_budget() < Quality::High.particle_budget());
        assert_eq!(Quality::parse("ALTA"), Some(Quality::High));
        assert_eq!(Quality::parse("x"), None);
    }
}
