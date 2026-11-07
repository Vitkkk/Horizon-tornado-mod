// Ready-made environments. Each is a plausible sounding for the storm type it is named after;
// the storms that form in it follow from its parameters, not from the name.

use super::sounding::EnvironmentSpec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// Plains supercells with a moderate tornado risk.
    Classic,
    /// High CAPE, strong low-level shear and helicity, low LCL: a tornado outbreak day.
    Outbreak,
    /// Very moist, slow storm-relative anvil winds: high-precipitation supercells.
    HighPrecip,
    /// Dry, high LCL, strong upper winds: low-precipitation supercells.
    LowPrecip,
    /// Strong shear, weak instability, strong linear forcing: a squall line / QLCS.
    Qlcs,
    /// Weak shear, moderate CAPE: pulse storms and multicells.
    Pulse,
    /// Inverted-V profile with a deep dry boundary layer: dry microbursts.
    DryMicroburst,
    /// Little instability: storms struggle.
    Marginal,
}

impl Preset {
    pub const ALL: [Preset; 8] = [
        Preset::Classic,
        Preset::Outbreak,
        Preset::HighPrecip,
        Preset::LowPrecip,
        Preset::Qlcs,
        Preset::Pulse,
        Preset::DryMicroburst,
        Preset::Marginal,
    ];

    pub fn parse(s: &str) -> Option<Preset> {
        Some(match s.to_ascii_lowercase().as_str() {
            "classic" | "classica" | "clássica" => Preset::Classic,
            "outbreak" | "surto" => Preset::Outbreak,
            "hp" | "highprecip" => Preset::HighPrecip,
            "lp" | "lowprecip" => Preset::LowPrecip,
            "qlcs" | "squall" | "line" => Preset::Qlcs,
            "pulse" | "multicell" => Preset::Pulse,
            "microburst" | "dry" => Preset::DryMicroburst,
            "marginal" => Preset::Marginal,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Preset::Classic => "classic",
            Preset::Outbreak => "outbreak",
            Preset::HighPrecip => "hp",
            Preset::LowPrecip => "lp",
            Preset::Qlcs => "qlcs",
            Preset::Pulse => "pulse",
            Preset::DryMicroburst => "microburst",
            Preset::Marginal => "marginal",
        }
    }

    pub fn spec(self) -> EnvironmentSpec {
        // Hodographs: (height m, speed m/s, direction FROM in degrees)
        match self {
            Preset::Classic => EnvironmentSpec {
                surface_temp_c: 29.0,
                surface_dewpoint_c: 20.0,
                surface_pressure_pa: 97_000.0,
                boundary_lapse: 8.5,
                mixed_layer_depth: 1100.0,
                cap_strength: 0.8,
                mid_lapse: 7.6,
                moist_depth: 1600.0,
                dry_aloft_depression: 16.0,
                tropopause: 12_500.0,
                hodograph: vec![
                    (0.0, 6.0, 160.0),
                    (1000.0, 13.0, 190.0),
                    (3000.0, 18.0, 225.0),
                    (6000.0, 26.0, 245.0),
                    (9000.0, 34.0, 250.0),
                    (12000.0, 40.0, 255.0),
                ],
            },
            Preset::Outbreak => EnvironmentSpec {
                surface_temp_c: 28.0,
                surface_dewpoint_c: 22.0,
                surface_pressure_pa: 96_800.0,
                boundary_lapse: 8.0,
                mixed_layer_depth: 800.0,
                cap_strength: 0.5,
                mid_lapse: 8.2,
                moist_depth: 1800.0,
                dry_aloft_depression: 18.0,
                tropopause: 13_000.0,
                hodograph: vec![
                    (0.0, 9.0, 150.0),
                    (500.0, 18.0, 175.0),
                    (1000.0, 23.0, 195.0),
                    (3000.0, 27.0, 225.0),
                    (6000.0, 32.0, 240.0),
                    (9000.0, 40.0, 245.0),
                    (12000.0, 50.0, 250.0),
                ],
            },
            Preset::HighPrecip => EnvironmentSpec {
                surface_temp_c: 28.0,
                surface_dewpoint_c: 24.0,
                surface_pressure_pa: 98_000.0,
                boundary_lapse: 7.5,
                mixed_layer_depth: 700.0,
                cap_strength: 0.4,
                mid_lapse: 6.8,
                moist_depth: 3500.0,
                dry_aloft_depression: 6.0,
                tropopause: 14_000.0,
                hodograph: vec![
                    (0.0, 6.0, 140.0),
                    (1000.0, 14.0, 170.0),
                    (3000.0, 16.0, 210.0),
                    (6000.0, 20.0, 230.0),
                    (9000.0, 18.0, 235.0),
                    (12000.0, 20.0, 240.0),
                ],
            },
            Preset::LowPrecip => EnvironmentSpec {
                surface_temp_c: 33.0,
                surface_dewpoint_c: 13.0,
                surface_pressure_pa: 90_000.0,
                boundary_lapse: 9.4,
                mixed_layer_depth: 2000.0,
                cap_strength: 0.6,
                mid_lapse: 8.6,
                moist_depth: 900.0,
                dry_aloft_depression: 22.0,
                tropopause: 12_500.0,
                hodograph: vec![
                    (0.0, 5.0, 150.0),
                    (1000.0, 9.0, 180.0),
                    (3000.0, 16.0, 230.0),
                    (6000.0, 28.0, 250.0),
                    (9000.0, 42.0, 255.0),
                    (12000.0, 55.0, 260.0),
                ],
            },
            Preset::Qlcs => EnvironmentSpec {
                surface_temp_c: 22.0,
                surface_dewpoint_c: 18.0,
                surface_pressure_pa: 98_500.0,
                boundary_lapse: 6.5,
                mixed_layer_depth: 500.0,
                cap_strength: 0.2,
                mid_lapse: 6.6,
                moist_depth: 2500.0,
                dry_aloft_depression: 8.0,
                tropopause: 12_000.0,
                hodograph: vec![
                    (0.0, 12.0, 180.0),
                    (1000.0, 25.0, 200.0),
                    (3000.0, 30.0, 220.0),
                    (6000.0, 33.0, 230.0),
                    (12000.0, 45.0, 240.0),
                ],
            },
            Preset::Pulse => EnvironmentSpec {
                surface_temp_c: 31.0,
                surface_dewpoint_c: 21.0,
                surface_pressure_pa: 100_000.0,
                boundary_lapse: 8.8,
                mixed_layer_depth: 1300.0,
                cap_strength: 0.3,
                mid_lapse: 6.4,
                moist_depth: 2500.0,
                dry_aloft_depression: 10.0,
                tropopause: 14_500.0,
                hodograph: vec![(0.0, 2.0, 200.0), (3000.0, 5.0, 240.0), (6000.0, 8.0, 260.0), (12000.0, 12.0, 270.0)],
            },
            Preset::DryMicroburst => EnvironmentSpec {
                surface_temp_c: 36.0,
                surface_dewpoint_c: 4.0,
                surface_pressure_pa: 85_000.0,
                boundary_lapse: 9.7,
                mixed_layer_depth: 3200.0,
                cap_strength: 0.0,
                mid_lapse: 8.0,
                // the deep well-mixed layer keeps the surface's low mixing ratio: an inverted V
                moist_depth: 3200.0,
                dry_aloft_depression: 6.0,
                tropopause: 13_000.0,
                hodograph: vec![(0.0, 4.0, 220.0), (3000.0, 8.0, 240.0), (6000.0, 12.0, 250.0), (12000.0, 18.0, 260.0)],
            },
            Preset::Marginal => EnvironmentSpec {
                surface_temp_c: 21.0,
                surface_dewpoint_c: 11.0,
                surface_pressure_pa: 99_500.0,
                boundary_lapse: 7.0,
                mixed_layer_depth: 900.0,
                cap_strength: 1.5,
                mid_lapse: 6.0,
                moist_depth: 1000.0,
                dry_aloft_depression: 12.0,
                tropopause: 11_500.0,
                hodograph: vec![(0.0, 5.0, 200.0), (3000.0, 12.0, 240.0), (6000.0, 18.0, 255.0), (12000.0, 28.0, 260.0)],
            },
        }
    }
}
