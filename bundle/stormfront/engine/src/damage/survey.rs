// Tornado records and the post-event damage survey.
//
// Every tornado keeps its track (positions, intensity, widths), what it destroyed and where.
// The survey rates it from damage only, as the Enhanced Fujita method does: for each damage
// indicator present along the path (frame houses, masonry, hardwood and softwood trees) it finds
// the highest degree of damage observed, takes that degree's expected wind, adjusted down or up
// for poor or good construction, and the rating is the highest of those. A violent vortex that
// crossed only open fields can therefore rate low or "EFU": the simulated wind is reported
// separately and never decides the rating.

use super::structures::DamageKind;
use crate::math::Vec3;
use crate::units::ms_to_kmh;
use std::collections::HashMap;

/// Survey cell size (blocks).
pub const CELL: i32 = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrackPoint {
    /// Simulation time (s) since the record began.
    pub t: f64,
    pub pos: Vec3,
    pub v_max: f64,
    pub r_max: f64,
    /// Radius of winds >= 29 m/s (the EF0 lower bound).
    pub r_damage: f64,
    pub translation: f64,
    pub morphology: u8,
}

/// What was found in one survey cell.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DamageCell {
    /// Structural blocks first seen in the cell, and how many failed, by kind.
    pub structural_seen: u32,
    pub kinds: [u32; 17],
    /// Leaves lost, trees uprooted/snapped/debarked, split by hardwood and softwood.
    pub hard_trees: [u32; 4],
    pub soft_trees: [u32; 4],
    pub scour_stage: u8,
    /// Highest wind (m/s) the engine evaluated on a block in the cell.
    pub max_wind: f32,
    pub quality: f32,
}

impl DamageCell {
    fn removed_structural(&self) -> u32 {
        [DamageKind::Glass, DamageKind::RoofCover, DamageKind::RoofStructure, DamageKind::Wall, DamageKind::Collapse, DamageKind::SweptFoundation, DamageKind::Contents]
            .iter()
            .map(|k| self.kinds[k.index()])
            .sum()
    }

    /// Frame residence (EF DI 2, FR12): (degree, expected wind m/s, description).
    pub fn house_dod(&self) -> Option<(u8, f64, &'static str)> {
        let seen = self.structural_seen.max(1) as f64;
        let k = |kind: DamageKind| self.kinds[kind.index()] as f64;
        let removed = self.removed_structural() as f64;
        if removed == 0.0 || self.structural_seen < 8 {
            return None;
        }
        let frac = removed / seen;
        let walls = (k(DamageKind::Wall) + k(DamageKind::Collapse)) / seen;
        let roof = k(DamageKind::RoofStructure) / seen;
        let cover = k(DamageKind::RoofCover) / seen;
        Some(if k(DamageKind::SweptFoundation) >= 2.0 && frac > 0.9 {
            (10, 89.0, "residência destruída; piso/fundação varridos")
        } else if frac >= 0.85 {
            (9, 76.0, "todas as paredes colapsadas")
        } else if frac >= 0.6 {
            (8, 68.0, "maioria das paredes colapsadas")
        } else if walls >= 0.1 {
            (7, 59.0, "paredes externas colapsadas")
        } else if roof >= 0.08 {
            (6, 55.0, "grandes seções da estrutura do telhado removidas")
        } else if cover >= 0.05 || roof > 0.0 {
            (4, 43.0, "perda significativa da cobertura (>20%)")
        } else if k(DamageKind::Glass) > 0.0 {
            (3, 43.0, "vidros de portas e janelas quebrados")
        } else if cover > 0.0 {
            (2, 35.0, "perda de cobertura do telhado (<20%)")
        } else {
            (1, 29.0, "limiar de dano visível")
        })
    }

    /// Masonry and engineered buildings (a simplified DI).
    pub fn masonry_dod(&self) -> Option<(u8, f64, &'static str)> {
        let failed = self.kinds[DamageKind::Engineered.index()] as f64;
        if failed < 2.0 {
            return None;
        }
        let frac = failed / self.structural_seen.max(1) as f64;
        Some(if frac >= 0.85 {
            (5, 89.0, "estrutura de alvenaria/concreto destruída")
        } else if frac >= 0.4 {
            (4, 74.0, "colapso de paredes de alvenaria")
        } else if frac >= 0.1 {
            (3, 61.0, "paredes de alvenaria parcialmente rompidas")
        } else {
            (2, 49.0, "danos em alvenaria")
        })
    }

    /// Trees (EF DIs 27 hardwood and 28 softwood).
    pub fn tree_dod(&self, soft: bool) -> Option<(u8, f64, &'static str)> {
        let t = if soft { self.soft_trees } else { self.hard_trees };
        let w = |hard: f64, s: f64| if soft { s } else { hard };
        if t[3] > 0 {
            Some((5, w(64.0, 62.0), "árvores descascadas, só tocos dos galhos maiores"))
        } else if t[2] > 0 {
            Some((4, w(49.0, 46.0), "troncos quebrados"))
        } else if t[1] > 0 {
            Some((3, w(41.0, 38.0), "árvores arrancadas"))
        } else if t[0] >= 40 {
            Some((2, w(33.0, 34.0), "galhos grandes quebrados"))
        } else if t[0] >= 8 {
            Some((1, 27.0, "galhos pequenos quebrados"))
        } else {
            None
        }
    }

    /// The best-supported wind estimate of the cell (m/s) and what it rests on.
    pub fn estimate(&self) -> Option<(f64, &'static str, u8, &'static str)> {
        let mut best: Option<(f64, &'static str, u8, &'static str)> = None;
        let mut consider = |di: &'static str, d: Option<(u8, f64, &'static str)>, adjust: bool| {
            if let Some((dod, wind, text)) = d {
                // construction quality moves the estimate towards the lower or upper bound
                let q = self.quality as f64;
                let wind = if adjust && q < 0.85 {
                    wind * 0.85
                } else if adjust && q > 1.15 {
                    wind * 1.1
                } else {
                    wind
                };
                if best.is_none_or(|b| wind > b.0) {
                    best = Some((wind, di, dod, text));
                }
            }
        };
        consider("residência de madeira (FR12)", self.house_dod(), true);
        consider("alvenaria/concreto", self.masonry_dod(), true);
        consider("árvores de madeira dura (TH)", self.tree_dod(false), false);
        consider("árvores coníferas (TS)", self.tree_dod(true), false);
        best
    }
}

/// EF rating from a damage-based wind estimate (m/s); None is EFU (no indicators).
pub fn ef_from_wind(v: f64) -> u8 {
    match v {
        v if v < 38.0 => 0,
        v if v < 49.0 => 1,
        v if v < 61.0 => 2,
        v if v < 74.0 => 3,
        v if v < 89.0 => 4,
        _ => 5,
    }
}

#[derive(Clone, Debug, Default)]
pub struct TornadoRecord {
    pub id: u32,
    pub formed_tick: u64,
    pub formed_day_time: i64,
    pub dissipated_tick: Option<u64>,
    pub track: Vec<TrackPoint>,
    pub path_length: f64,
    pub max_width: f64,
    pub max_sim_wind: f64,
    pub max_local_wind: f64,
    pub max_translation: f64,
    pub kinds: [u64; 17],
    pub blocks_destroyed: u64,
    pub cells: HashMap<(i32, i32), DamageCell>,
    pub debris_launched: u64,
    pub debris_fragmented: u64,
    pub debris_max_distance: f64,
    pub debarked_blocks: u64,
    pub scour_columns: [u64; 4],
    time: f64,
}

impl TornadoRecord {
    pub fn new(id: u32, tick: u64, day_time: i64) -> Self {
        TornadoRecord { id, formed_tick: tick, formed_day_time: day_time, ..Default::default() }
    }

    pub fn cell_key(x: i32, z: i32) -> (i32, i32) {
        (x.div_euclid(CELL), z.div_euclid(CELL))
    }

    pub fn cell(&mut self, x: i32, z: i32) -> &mut DamageCell {
        let q = super::structures::building_quality(x, z) as f32;
        self.cells.entry(Self::cell_key(x, z)).or_insert_with(|| DamageCell { quality: q, ..Default::default() })
    }

    /// Record the tornado's state (every second or so).
    pub fn sample(&mut self, dt: f64, p: TrackPoint) {
        self.time += dt;
        if let Some(last) = self.track.last() {
            self.path_length += (p.pos - last.pos).horizontal_length();
        }
        self.max_width = self.max_width.max(2.0 * p.r_damage);
        self.max_sim_wind = self.max_sim_wind.max(p.v_max);
        self.max_translation = self.max_translation.max(p.translation);
        self.track.push(TrackPoint { t: self.time, ..p });
    }

    pub fn damage(&mut self, pos: [i32; 3], kind: DamageKind, wind: f64) {
        self.kinds[kind.index()] += 1;
        if !matches!(kind, DamageKind::ScourLight | DamageKind::ScourModerate | DamageKind::ScourExtreme | DamageKind::Debarked) {
            self.blocks_destroyed += 1;
        }
        if kind == DamageKind::Debarked {
            self.debarked_blocks += 1;
        }
        self.max_local_wind = self.max_local_wind.max(wind);
        let c = self.cell(pos[0], pos[2]);
        c.kinds[kind.index()] += 1;
        c.max_wind = c.max_wind.max(wind as f32);
    }

    pub fn tree(&mut self, pos: [i32; 3], soft: bool, what: usize, wind: f64) {
        self.max_local_wind = self.max_local_wind.max(wind);
        let c = self.cell(pos[0], pos[2]);
        if soft {
            c.soft_trees[what] += 1;
        } else {
            c.hard_trees[what] += 1;
        }
    }

    pub fn scour(&mut self, x: i32, z: i32, stage: u8) {
        if (1..=4).contains(&stage) {
            self.scour_columns[stage as usize - 1] += 1;
        }
        let c = self.cell(x, z);
        c.scour_stage = c.scour_stage.max(stage);
    }

    pub fn observed_structure(&mut self, x: i32, z: i32, blocks: u32) {
        self.cell(x, z).structural_seen += blocks;
    }

    pub fn mean_translation(&self) -> f64 {
        if self.track.len() < 2 {
            return 0.0;
        }
        self.track.iter().map(|p| p.translation).sum::<f64>() / self.track.len() as f64
    }

    pub fn duration(&self) -> f64 {
        self.track.last().map_or(0.0, |p| p.t)
    }

    /// The damage-based rating: (EF or None for EFU, wind estimate, indicator, degree, text).
    pub fn rating(&self) -> (Option<u8>, f64, &'static str, u8, &'static str) {
        let best = self.cells.values().filter_map(|c| c.estimate()).max_by(|a, b| a.0.total_cmp(&b.0));
        match best {
            Some((w, di, dod, text)) => (Some(ef_from_wind(w)), w, di, dod, text),
            None => (None, 0.0, "nenhum indicador de dano", 0, ""),
        }
    }

    /// The post-event report, as lines of text, with an ASCII damage map.
    pub fn report(&self, map_width: usize) -> Vec<String> {
        let (ef, wind, di, dod, text) = self.rating();
        let mut out = Vec::new();
        out.push(format!("=== Levantamento pós-evento: tornado #{} ===", self.id));
        let fmt_time = |s: f64| format!("{}min {:02}s", (s / 60.0) as u64, (s % 60.0) as u64);
        out.push(format!(
            "Formação: tick {} (hora do dia {:02}:{:02}); {}",
            self.formed_tick,
            ((self.formed_day_time.rem_euclid(24000) / 1000 + 6) % 24),
            (self.formed_day_time.rem_euclid(1000) * 60 / 1000),
            match self.dissipated_tick {
                Some(t) => format!("dissipação: tick {t}"),
                None => "ainda ativo".to_string(),
            }
        ));
        if let (Some(a), Some(b)) = (self.track.first(), self.track.last()) {
            out.push(format!(
                "Trajeto: ({:.0}, {:.0}) -> ({:.0}, {:.0}); comprimento {:.0} m; duração {}",
                a.pos.x, a.pos.z, b.pos.x, b.pos.z, self.path_length, fmt_time(self.duration())
            ));
        }
        out.push(format!(
            "Largura máxima do campo de ventos danosos (>= 29 m/s): {:.0} m; translação média {:.1} m/s ({:.0} km/h), máx {:.1} m/s",
            self.max_width,
            self.mean_translation(),
            ms_to_kmh(self.mean_translation()),
            self.max_translation
        ));
        out.push(format!(
            "Vento simulado máximo do vórtice: {:.0} m/s ({:.0} km/h), classe de simulação T{}; maior vento avaliado num bloco: {:.0} m/s",
            self.max_sim_wind,
            ms_to_kmh(self.max_sim_wind),
            crate::tornado::wind_class(self.max_sim_wind),
            self.max_local_wind
        ));
        match ef {
            Some(ef) => out.push(format!(
                "Classificação por danos: EF{ef} (estimativa {wind:.0} m/s = {:.0} km/h) - indicador: {di}, grau de dano {dod}: {text}",
                ms_to_kmh(wind)
            )),
            None => out.push("Classificação por danos: EFU (sem indicadores de dano no trajeto)".to_string()),
        }
        let kinds: Vec<String> = DamageKind::ALL
            .iter()
            .filter(|k| self.kinds[k.index()] > 0)
            .map(|k| format!("{} {}", k.name(), self.kinds[k.index()]))
            .collect();
        out.push(format!("Danos: {} blocos destruídos; {}", self.blocks_destroyed, kinds.join(", ")));
        out.push(format!(
            "Scouring (colunas): leve {}, solo superficial {}, escavação {}, sulcos extremos {}; descascamento: {} blocos de tronco",
            self.scour_columns[0], self.scour_columns[1], self.scour_columns[2], self.scour_columns[3], self.debarked_blocks
        ));
        out.push(format!(
            "Detritos: {} lançados, {} fragmentações, maior distância {:.0} m",
            self.debris_launched, self.debris_fragmented, self.debris_max_distance
        ));
        out.extend(self.map(map_width));
        out
    }

    /// An ASCII map of the damage: ' ' none, '.' EF0, ':' EF1, '+' EF2, 'x' EF3, 'X' EF4, '#' EF5
    /// (by each cell's damage estimate); '~' scouring without indicators; 'd' debarking.
    pub fn map(&self, width: usize) -> Vec<String> {
        if self.cells.is_empty() {
            return vec!["(sem células de dano)".to_string()];
        }
        let (mut x0, mut x1, mut z0, mut z1) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
        for &(x, z) in self.cells.keys() {
            x0 = x0.min(x);
            x1 = x1.max(x);
            z0 = z0.min(z);
            z1 = z1.max(z);
        }
        let span = ((x1 - x0 + 1).max(z1 - z0 + 1) as usize).max(1);
        let step = span.div_ceil(width.max(8)).max(1) as i32;
        let mut out = vec![format!(
            "Mapa de danos (1 caractere = {} m; norte em cima; origem {}, {}):",
            step * CELL,
            x0 * CELL,
            z0 * CELL
        )];
        let mut z = z0;
        while z <= z1 {
            let mut line = String::new();
            let mut x = x0;
            while x <= x1 {
                let mut best: Option<f64> = None;
                let mut scour = 0u8;
                let mut debark = false;
                for dz in 0..step {
                    for dx in 0..step {
                        if let Some(c) = self.cells.get(&(x + dx, z + dz)) {
                            if let Some((w, ..)) = c.estimate() {
                                best = Some(best.map_or(w, |b: f64| b.max(w)));
                            }
                            scour = scour.max(c.scour_stage);
                            debark |= c.hard_trees[3] + c.soft_trees[3] > 0;
                        }
                    }
                }
                line.push(match best {
                    _ if debark => 'd',
                    Some(w) => ['.', ':', '+', 'x', 'X', '#'][ef_from_wind(w) as usize],
                    None if scour >= 2 => '~',
                    None if scour == 1 => ',',
                    None => ' ',
                });
                x += step;
            }
            out.push(line.trim_end().to_string());
            z += step;
        }
        out
    }

    /// The track as CSV (time, x, z, v_max, r_max, r_damage, translation, morphology).
    pub fn track_csv(&self) -> String {
        let mut s = String::from("t_s,x,z,v_max_ms,r_max_m,r_damage_m,translation_ms,morphology\n");
        for p in &self.track {
            s.push_str(&format!("{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{}\n", p.t, p.pos.x, p.pos.z, p.v_max, p.r_max, p.r_damage, p.translation, p.morphology));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn violent_vortex_over_open_fields_is_efu_or_low() {
        let mut r = TornadoRecord::new(1, 0, 0);
        r.sample(1.0, TrackPoint { v_max: 95.0, r_damage: 200.0, ..Default::default() });
        for x in 0..50 {
            r.damage([x, 64, 0], DamageKind::Vegetation, 90.0);
            r.scour(x, 0, 3);
        }
        let (ef, ..) = r.rating();
        assert_eq!(ef, None, "no damage indicators: EFU despite a 95 m/s vortex");
        assert!(r.report(40).iter().any(|l| l.contains("EFU")));
    }

    #[test]
    fn destroyed_frame_houses_rate_high_and_glass_only_rates_low() {
        let mut house = TornadoRecord::new(2, 0, 0);
        house.observed_structure(4, 4, 200);
        for i in 0..190 {
            let kind = if i < 120 { DamageKind::Wall } else { DamageKind::RoofStructure };
            house.damage([4, 66, 4], kind, 80.0);
        }
        let (ef, w, ..) = house.rating();
        assert!(ef.unwrap() >= 4, "{ef:?} {w}");

        let mut glass = TornadoRecord::new(3, 0, 0);
        glass.observed_structure(4, 4, 200);
        for _ in 0..6 {
            glass.damage([4, 66, 4], DamageKind::Glass, 45.0);
        }
        let (ef, ..) = glass.rating();
        assert_eq!(ef, Some(1));
    }

    #[test]
    fn debarked_trees_are_an_ef3_to_ef4_indicator() {
        let mut r = TornadoRecord::new(4, 0, 0);
        r.tree([0, 65, 0], false, 3, 85.0);
        let (ef, ..) = r.rating();
        assert_eq!(ef, Some(3));
        let csv = r.track_csv();
        assert!(csv.starts_with("t_s,x,z"));
    }

    #[test]
    fn map_marks_damage_cells() {
        let mut r = TornadoRecord::new(5, 0, 0);
        r.observed_structure(0, 0, 50);
        for _ in 0..50 {
            r.damage([0, 66, 0], DamageKind::Wall, 70.0);
        }
        r.scour(40, 0, 2);
        let m = r.map(20);
        assert!(m.iter().skip(1).any(|l| l.contains('X') || l.contains('#') || l.contains('x')), "{m:?}");
        assert!(m.iter().any(|l| l.contains('~')));
    }
}
