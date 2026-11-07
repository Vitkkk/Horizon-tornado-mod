// The server half: the authoritative weather, damage and tornado records of one world, advanced
// once per server tick of the Overworld, and what each player's client half is sent.
//
// It knows the host only through `WorldIo` (column reads and bulk edits) and the plain player
// list the glue gives it, so it is tested here against an in-memory world.

use crate::atmosphere::Environment;
use crate::atmosphere::presets::Preset;
use crate::commands::{SERVER_HELP, ServerCommand, parse_server};
use crate::config::ServerSettings;
use crate::damage::engine::{Budget, DamageEngine, Effect};
use crate::damage::survey::{TornadoRecord, TrackPoint};
use crate::damage::world::WorldIo;
use crate::math::Vec3;
use crate::net::{self, Fx};
use crate::storms::StormKind;
use crate::storms::system::{StormSystem, WeatherEvent};
use crate::units::{TICK, ms_to_kmh};
use std::collections::HashMap;

/// A player as the server half sees one.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerView {
    pub uuid: u128,
    pub name: String,
    pub pos: Vec3,
    pub dimension: String,
    pub permission: u8,
}

/// A message for a player's client half.
#[derive(Clone, Debug, PartialEq)]
pub struct Outgoing {
    pub to: u128,
    pub data: Vec<u8>,
}

/// What a command did: lines for the player, and whether it succeeded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommandOutcome {
    pub lines: Vec<String>,
    pub ok: bool,
}

pub const OVERWORLD: &str = "minecraft:overworld";

pub struct ServerSim {
    pub settings: ServerSettings,
    pub weather: StormSystem,
    pub damage: DamageEngine,
    pub records: HashMap<u32, TornadoRecord>,
    pub finished: Vec<TornadoRecord>,
    /// Files the glue should write (name relative to the mod's folder, contents).
    pub files: Vec<(String, String)>,
    pub budget: Budget,
    pub tick: u64,
    pub day_time: i64,
    fx: Vec<Fx>,
    tracks_requested: Vec<(u128, u32)>,
    ground_known: bool,
}

impl ServerSim {
    pub fn new(seed: u64) -> Self {
        let settings = ServerSettings::default();
        ServerSim {
            weather: StormSystem::new(Environment::from_preset(Preset::Classic), settings.scale, seed),
            damage: DamageEngine::new(&settings, seed),
            settings,
            records: HashMap::new(),
            finished: Vec::new(),
            files: Vec::new(),
            budget: Budget::default(),
            tick: 0,
            day_time: 0,
            fx: Vec::new(),
            tracks_requested: Vec::new(),
            ground_known: false,
        }
    }

    fn active(&self) -> bool {
        !self.weather.storms.is_empty() || !self.weather.tornadoes.is_empty() || self.weather.natural || !self.damage.debris.pieces.is_empty()
    }

    /// One server tick of the Overworld. `players` are those in it.
    pub fn tick(&mut self, io: &mut dyn WorldIo, players: &[PlayerView], day_time: i64) -> Vec<Outgoing> {
        self.tick += 1;
        self.day_time = day_time;
        let mut out = Vec::new();
        if !self.ground_known {
            if let Some(p) = players.first() {
                self.weather.ground_y = p.pos.y.floor();
                self.ground_known = true;
            }
        }
        if self.active() {
            self.weather.step(TICK);
            // tornadoes stand on the ground the damage engine has read under them
            for t in &mut self.weather.tornadoes {
                let (x, z) = (t.pos.x.floor() as i32, t.pos.z.floor() as i32);
                if let Some(c) = self.damage.window.column(x, z) {
                    let g = c.ground as f64 + 1.0;
                    t.pos.y += (g - t.pos.y) * 0.2;
                } else if t.pos.y.abs() < 1e-9 || (t.pos.y - self.weather.ground_y).abs() > 200.0 {
                    t.pos.y = self.weather.ground_y;
                }
            }
            for t in &self.weather.tornadoes {
                self.records.entry(t.id).or_insert_with(|| TornadoRecord::new(t.id, self.tick, day_time));
            }
            self.damage.tick(io, &mut self.weather, &mut self.records, TICK, &self.settings, self.budget);
            if self.tick % 20 == 0 {
                for t in &self.weather.tornadoes {
                    if let Some(r) = self.records.get_mut(&t.id) {
                        r.sample(1.0, TrackPoint {
                            t: 0.0,
                            pos: t.pos,
                            v_max: t.params.v_max,
                            r_max: t.params.r_max,
                            r_damage: t.radius_of_wind(29.0),
                            translation: t.velocity.horizontal_length(),
                            morphology: t.morphology().id(),
                        });
                    }
                }
            }
            self.handle_events();
            self.collect_effects();
        }
        self.sync(players, &mut out);
        out
    }

    fn handle_events(&mut self) {
        let events = std::mem::take(&mut self.weather.events);
        for e in events {
            match e {
                WeatherEvent::Lightning { pos, .. } => self.fx.push(Fx::Lightning { pos }),
                WeatherEvent::TornadoFormed { id, pos, .. } => {
                    self.fx.push(Fx::Notice { text: format!("[Stormfront] Tornado #{id} formou-se em ({:.0}, {:.0})", pos.x, pos.z) });
                }
                WeatherEvent::TornadoDissipated { id, .. } => {
                    if let Some(mut r) = self.records.remove(&id) {
                        r.dissipated_tick = Some(self.tick);
                        let (ef, ..) = r.rating();
                        let rating = ef.map_or("EFU".to_string(), |e| format!("EF{e}"));
                        self.fx.push(Fx::Notice {
                            text: format!("[Stormfront] Tornado #{id} dissipou: {:.1} km, avaliação preliminar {rating}. /tornado survey {id}", r.path_length / 1000.0),
                        });
                        self.files.push((format!("tornado-{id}.txt"), r.report(80).join("\n")));
                        self.files.push((format!("tornado-{id}-track.csv"), r.track_csv()));
                        self.finished.push(r);
                        if self.finished.len() > 32 {
                            self.finished.remove(0);
                        }
                    }
                }
                WeatherEvent::StormFormed { .. } => {}
            }
        }
    }

    fn collect_effects(&mut self) {
        for e in self.damage.effects.drain(..) {
            self.fx.push(match e {
                Effect::Burst { pos, state, vel } => Fx::Burst { pos, vel, state },
                Effect::TreeFall { pos } => Fx::TreeFall { pos },
                Effect::Collapse { pos, blocks } => Fx::Collapse { pos, blocks },
                Effect::Fragment { pos, state, pieces } => Fx::Fragment { pos, pieces, state },
            });
        }
        if self.fx.len() > 600 {
            let n = self.fx.len() - 600;
            self.fx.drain(..n);
        }
    }

    fn sync(&mut self, players: &[PlayerView], out: &mut Vec<Outgoing>) {
        let weather_due = self.tick % self.settings.sync_interval_ticks.max(1) as u64 == 0;
        let debris_due = self.tick % self.settings.debris_sync_interval_ticks.max(1) as u64 == 0;
        let weather = weather_due.then(|| net::encode_weather(self.tick, &self.weather.env, &self.weather.storms, &self.weather.tornadoes, self.weather.ground_y));
        let fx = std::mem::take(&mut self.fx);
        for p in players {
            if let Some(w) = &weather {
                out.push(Outgoing { to: p.uuid, data: w.clone() });
            }
            if debris_due && !self.damage.debris.pieces.is_empty() {
                let near: Vec<(u32, Vec3, Vec3, f64, &str)> = self
                    .damage
                    .debris
                    .pieces
                    .iter()
                    .filter(|d| (d.pos - p.pos).length() < 900.0)
                    .map(|d| (d.id, d.pos, d.vel, d.size, d.state.as_str()))
                    .collect();
                for m in net::encode_debris(self.tick, &near).into_iter().take(3) {
                    out.push(Outgoing { to: p.uuid, data: m });
                }
            }
            let mine: Vec<Fx> = fx
                .iter()
                .filter(|f| match f {
                    Fx::Burst { pos, .. } | Fx::Fragment { pos, .. } => (*pos - p.pos).length() < 300.0,
                    Fx::TreeFall { pos } | Fx::Collapse { pos, .. } => (*pos - p.pos).length() < 600.0,
                    Fx::Lightning { pos } => (*pos - p.pos).horizontal_length() < 12_000.0,
                    Fx::Notice { .. } => true,
                })
                .cloned()
                .collect();
            if !mine.is_empty() {
                for m in net::encode_effects(self.tick, &mine).into_iter().take(4) {
                    out.push(Outgoing { to: p.uuid, data: m });
                }
            }
        }
        for (to, id) in std::mem::take(&mut self.tracks_requested) {
            let r = self.records.get(&id).or_else(|| self.finished.iter().find(|r| r.id == id));
            if let Some(r) = r {
                let pts: Vec<(f64, f64, f64)> = r.track.iter().map(|p| (p.pos.x, p.pos.z, p.v_max)).collect();
                out.push(Outgoing { to, data: net::encode_track(id, &pts) });
            }
        }
        // live tracks follow along every few seconds
        if self.tick % 60 == 0 {
            for r in self.records.values() {
                let pts: Vec<(f64, f64, f64)> = r.track.iter().map(|p| (p.pos.x, p.pos.z, p.v_max)).collect();
                if pts.len() >= 2 {
                    let m = net::encode_track(r.id, &pts);
                    for p in players {
                        out.push(Outgoing { to: p.uuid, data: m.clone() });
                    }
                }
            }
        }
    }

    /// A player's `/storm` or `/tornado` command. None when it is not ours.
    pub fn command(&mut self, line: &str, player: &PlayerView) -> Option<CommandOutcome> {
        let parsed = parse_server(line)?;
        let cmd = match parsed {
            Ok(c) => c,
            Err(e) => return Some(CommandOutcome { lines: vec![e], ok: false }),
        };
        if player.dimension != OVERWORLD && !matches!(cmd, ServerCommand::Help | ServerCommand::List | ServerCommand::Survey(_) | ServerCommand::Debug) {
            return Some(CommandOutcome { lines: vec!["Stormfront simula o clima apenas no Overworld.".into()], ok: false });
        }
        if !self.ground_known {
            self.weather.ground_y = player.pos.y.floor();
            self.ground_known = true;
        }
        let mut lines = Vec::new();
        let ok = true;
        match cmd {
            ServerCommand::Help => lines.extend(SERVER_HELP.iter().map(|s| s.to_string())),
            ServerCommand::Storm { kind, microburst, distance } => {
                let d = distance.unwrap_or(if microburst { 0.0 } else { 3500.0 });
                let id = self.weather.spawn_storm(kind, player.pos, d);
                if microburst {
                    if let Some(s) = self.weather.storms.iter_mut().find(|s| s.id == id) {
                        s.age = s.life * 0.86;
                        s.pos = player.pos + Vec3::new(250.0, 0.0, 0.0);
                        s.step(0.0, &self.weather.env, 0.0);
                    }
                }
                let s = self.weather.storms.iter().find(|s| s.id == id).unwrap();
                lines.push(format!(
                    "Tempestade #{id}: {} a {:.0} m, movendo-se a {:.1} m/s ({:.0} km/h) para {:.0}°",
                    s.kind.name(),
                    (s.pos - player.pos).horizontal_length(),
                    s.motion.horizontal_length(),
                    ms_to_kmh(s.motion.horizontal_length()),
                    heading(s.motion)
                ));
                if s.kind.is_supercell() {
                    let odds = StormSystem::genesis_odds(&self.weather.env, 0.85, s.kind);
                    lines.push(format!(
                        "Ambiente: STP {:.1}; com mesociclone forte, chance de tornadogênese ~{:.0}%/min, potencial {:.0} m/s",
                        self.weather.env.stp,
                        odds.per_minute * 100.0,
                        odds.potential
                    ));
                }
                if kind == StormKind::Single && !microburst && self.weather.env.shear_0_6 > 18.0 {
                    lines.push("Nota: este ambiente favorece supercélulas; células comuns duram pouco nele.".into());
                }
            }
            ServerCommand::Env(p) => {
                self.weather.set_preset(p);
                lines.extend(self.weather.env.summary_lines());
            }
            ServerCommand::Clear => {
                self.weather.clear();
                self.damage.debris.clear();
                lines.push("Tempestades, tornados e detritos removidos.".into());
            }
            ServerCommand::Debug => {
                lines.extend(self.weather.env.summary_lines());
                for s in &self.weather.storms {
                    lines.push(format!(
                        "#{} {} [{}] a {:.0} m; updraft {:.0} m/s, precip {:.2}, granizo {:.1} cm, meso {:.2} (Vrot {:.0} m/s, oclusão {:.2}), tornados {}",
                        s.id,
                        s.kind.name(),
                        s.stage.name(),
                        (s.pos - player.pos).horizontal_length(),
                        s.updraft,
                        s.precip,
                        s.hail_size,
                        s.meso.strength,
                        s.meso.v_rot,
                        s.meso.occlusion,
                        s.tornadoes_produced
                    ));
                }
                for t in &self.weather.tornadoes {
                    lines.push(tornado_line(t, player.pos));
                }
                let r = self.damage.last;
                lines.push(format!(
                    "Dano: {} colunas em cache, lidas {} / avaliadas {} / blocos {} / falhas {} no último tick; detritos {}",
                    self.damage.window.column_count(),
                    r.columns_read,
                    r.columns_evaluated,
                    r.blocks_checked,
                    r.blocks_failed,
                    self.damage.debris.pieces.len()
                ));
            }
            ServerCommand::Natural(on) => {
                self.weather.natural = on;
                lines.push(format!("Formação natural de tempestades: {}", if on { "ligada" } else { "desligada" }));
            }
            ServerCommand::TimeScale(x) => {
                self.weather.scale.storm_time = x;
                self.settings.scale.storm_time = x;
                lines.push(format!("Escala de tempo das tempestades: {x}x"));
            }
            ServerCommand::ForceTornado => {
                let near = self.weather.storms.iter_mut().filter(|s| s.kind.is_supercell() || s.kind == StormKind::Qlcs).min_by(|a, b| (a.pos - player.pos).horizontal_length().total_cmp(&(b.pos - player.pos).horizontal_length()));
                match near {
                    Some(s) => {
                        s.force_tornado = true;
                        s.meso.strength = s.meso.strength.max(0.8);
                        lines.push(format!("Tempestade #{} produzirá um tornado sob o mesociclone.", s.id));
                    }
                    None => lines.push("Nenhuma supercélula/QLCS ativa.".into()),
                }
            }
            ServerCommand::TornadoSpawn(shape) => {
                let at = player.pos + Vec3::new(-120.0, 0.0, 0.0);
                let id = self.weather.spawn_tornado(shape, Vec3::new(at.x, self.weather.ground_y, at.z));
                lines.push(format!("Tornado #{id} ({shape:?}) a 120 m a oeste; use /tornado intensity|speed|multivortex para ajustá-lo."));
            }
            ServerCommand::Intensity(v) => {
                match self.weather.nearest_tornado(player.pos) {
                    Some(t) => {
                        t.overrides.v_max = Some(v);
                        t.target_v_max = v;
                        lines.push(format!("Tornado #{}: intensidade alvo {v:.0} m/s ({:.0} km/h)", t.id, ms_to_kmh(v)));
                    }
                    None => lines.push("Nenhum tornado ativo.".into()),
                }
            }
            ServerCommand::Multivortex(on) => match self.weather.nearest_tornado(player.pos) {
                Some(t) => {
                    t.overrides.multivortex = Some(on);
                    t.multivortex.enabled = on;
                    lines.push(format!("Tornado #{}: múltiplos vórtices {}", t.id, if on { "ligados" } else { "desligados" }));
                }
                None => lines.push("Nenhum tornado ativo.".into()),
            },
            ServerCommand::Speed(v) => match self.weather.nearest_tornado(player.pos) {
                Some(t) => {
                    t.overrides.speed = Some(v);
                    lines.push(format!("Tornado #{}: translação {v:.1} m/s ({:.0} km/h)", t.id, ms_to_kmh(v)));
                }
                None => lines.push("Nenhum tornado ativo.".into()),
            },
            ServerCommand::Damage(on) => {
                self.settings.damage = on;
                lines.push(format!("Danos ao mundo: {}", if on { "ligados" } else { "desligados" }));
            }
            ServerCommand::Dissipate => match self.weather.nearest_tornado(player.pos) {
                Some(t) => {
                    t.overrides = Default::default();
                    t.life = t.age.min(t.life);
                    t.stage = crate::tornado::TornadoStage::Shrinking;
                    lines.push(format!("Tornado #{} entrando em dissipação.", t.id));
                }
                None => lines.push("Nenhum tornado ativo.".into()),
            },
            ServerCommand::List => {
                if self.weather.tornadoes.is_empty() && self.finished.is_empty() {
                    lines.push("Nenhum tornado registrado.".into());
                }
                for t in &self.weather.tornadoes {
                    lines.push(tornado_line(t, player.pos));
                }
                for r in self.finished.iter().rev().take(8) {
                    let (ef, ..) = r.rating();
                    lines.push(format!(
                        "#{} (dissipado): {:.1} km, largura máx {:.0} m, {}",
                        r.id,
                        r.path_length / 1000.0,
                        r.max_width,
                        ef.map_or("EFU".to_string(), |e| format!("EF{e}"))
                    ));
                }
            }
            ServerCommand::Survey(id) => {
                let id = id.or_else(|| self.weather.tornadoes.first().map(|t| t.id)).or_else(|| self.finished.last().map(|r| r.id));
                let rec = id.and_then(|id| self.records.get(&id).or_else(|| self.finished.iter().find(|r| r.id == id)));
                match rec {
                    Some(r) => {
                        let report = r.report(56);
                        self.files.push((format!("tornado-{}.txt", r.id), r.report(80).join("\n")));
                        self.files.push((format!("tornado-{}-track.csv", r.id), r.track_csv()));
                        lines.extend(report);
                        lines.push(format!("Relatório salvo em config/stormfront/tornado-{}.txt", r.id));
                    }
                    None => lines.push("Nenhum tornado para levantar.".into()),
                }
            }
            ServerCommand::Track(id) => {
                let id = id.or_else(|| self.weather.tornadoes.first().map(|t| t.id)).or_else(|| self.finished.last().map(|r| r.id));
                match id {
                    Some(id) => {
                        self.tracks_requested.push((player.uuid, id));
                        lines.push(format!("Trajetória do tornado #{id} enviada; /sfx track on para vê-la."));
                    }
                    None => lines.push("Nenhuma trajetória registrada.".into()),
                }
            }
        }
        Some(CommandOutcome { lines, ok })
    }
}

/// Compass heading (degrees, 0 north, clockwise) of a world vector.
pub fn heading(v: Vec3) -> f64 {
    v.x.atan2(-v.z).to_degrees().rem_euclid(360.0)
}

fn tornado_line(t: &crate::tornado::Tornado, from: Vec3) -> String {
    format!(
        "Tornado #{} [{}] {} a {:.0} m: Vmax {:.0} m/s ({:.0} km/h, T{}), Rmax {:.0} m, ventos danosos até {:.0} m, translação {:.1} m/s, detritos {:.0} kg",
        t.id,
        t.stage.name(),
        t.morphology().name(),
        (t.pos - from).horizontal_length(),
        t.params.v_max,
        ms_to_kmh(t.params.v_max),
        crate::tornado::wind_class(t.peak_wind()),
        t.params.r_max,
        t.radius_of_wind(29.0),
        t.velocity.horizontal_length(),
        t.debris_load
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage::world::testing::MemoryWorld;

    fn player() -> PlayerView {
        PlayerView { uuid: 7, name: "tester".into(), pos: Vec3::new(0.0, 65.0, 0.0), dimension: OVERWORLD.into(), permission: 4 }
    }

    #[test]
    fn commands_spawn_and_survey_and_messages_flow() {
        let mut sim = ServerSim::new(1);
        let mut world = MemoryWorld::flat(64);
        world.house(-60, -3);
        let p = player();
        let out = sim.command("storm env outbreak", &p).unwrap();
        assert!(out.lines[0].contains("SBCAPE"));
        let out = sim.command("tornado spawn wedge", &p).unwrap();
        assert!(out.lines[0].contains("Tornado #1"));
        sim.command("tornado intensity 90", &p).unwrap();
        sim.command("tornado speed 10", &p).unwrap();
        let mut got_weather = false;
        let mut got_debris = false;
        for _ in 0..(20 * 12) {
            for m in sim.tick(&mut world, std::slice::from_ref(&p), 6000) {
                assert_eq!(m.to, 7);
                assert!(m.data.len() < 30_000);
                match net::kind_of(&m.data) {
                    Some(net::KIND_WEATHER) => got_weather = true,
                    Some(net::KIND_DEBRIS) => got_debris = true,
                    _ => {}
                }
            }
        }
        assert!(got_weather);
        assert!(got_debris, "the wedge threw debris");
        let survey = sim.command("tornado survey", &p).unwrap();
        assert!(survey.lines.iter().any(|l| l.contains("Classificação por danos")), "{:?}", survey.lines);
        assert!(sim.files.iter().any(|(n, _)| n == "tornado-1.txt"));
        let list = sim.command("tornado list", &p).unwrap();
        assert!(list.lines[0].starts_with("Tornado #1"));
        assert!(sim.command("say hi", &p).is_none());
    }

    #[test]
    fn nothing_happens_and_nothing_is_sent_for_empty_weather_beyond_sync() {
        let mut sim = ServerSim::new(2);
        let mut world = MemoryWorld::flat(64);
        let out = sim.tick(&mut world, &[player()], 0);
        assert!(out.iter().all(|m| net::kind_of(&m.data) != Some(net::KIND_DEBRIS)));
        assert_eq!(world.reads, 0);
    }
}
