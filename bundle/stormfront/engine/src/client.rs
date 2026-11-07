// The client half: a replica of the server's weather (rebuilt from its messages and moved on
// between them), the particles drawn from it, the debris the server reports, the radar and its
// panel, the scientific overlay, the trajectory view, sounds, camera shake and warnings. Nothing
// here changes the world.

use crate::atmosphere::Environment;
use crate::atmosphere::presets::Preset;
use crate::audio::{SoundCue, break_sound, event, roar, thunder};
use crate::commands::{CLIENT_HELP, ClientCommand, parse_client};
use crate::config::{ClientSettings, Quality, WorldScale};
use crate::hud::{HudOp, RadarView, radar_panel};
use crate::math::{Vec3, smoothstep};
use crate::net::{self, DebrisView, Fx};
use crate::particles::{ParticleSystem, block_color, emitters};
use crate::radar::Radar;
use crate::radar::scanning::RadarSite;
use crate::render_bridge::{self, Frame, Instance, Shape};
use crate::storms::system::StormSystem;
use crate::units::{TICK, ms_to_kmh};
use std::collections::{HashMap, VecDeque};

pub struct ClientState {
    pub settings: ClientSettings,
    pub weather: StormSystem,
    env: Option<Environment>,
    scale: WorldScale,
    pub debris: Vec<DebrisView>,
    debris_age: f64,
    pub particles: ParticleSystem,
    pub radar: Option<Radar>,
    pub radar_open: bool,
    pub view: RadarView,
    radar_site: Option<Vec3>,
    pub tracks: HashMap<u32, Vec<(f64, f64, f64)>>,
    pub show_track: bool,
    sub_tracks: HashMap<(u32, usize), VecDeque<Vec3>>,
    /// Lines for the chat, drained by the glue.
    pub chat: Vec<String>,
    /// Sounds to play, drained by the glue.
    pub sounds: Vec<SoundCue>,
    pub player: Vec3,
    pub tick: u64,
    last_weather_tick: u64,
    roar_timer: u32,
    rain_timer: u32,
    shake: f32,
    instances: Vec<Instance>,
    pub received: u64,
}

impl ClientState {
    pub fn new(settings: ClientSettings) -> Self {
        let scale = WorldScale::default();
        ClientState {
            particles: ParticleSystem::new(settings.quality, 0x5F0),
            settings,
            weather: StormSystem::new(Environment::from_preset(Preset::Classic), scale, 1),
            env: None,
            scale,
            debris: Vec::new(),
            debris_age: 0.0,
            radar: None,
            radar_open: false,
            view: RadarView::default(),
            radar_site: None,
            tracks: HashMap::new(),
            show_track: false,
            sub_tracks: HashMap::new(),
            chat: Vec::new(),
            sounds: Vec::new(),
            player: Vec3::ZERO,
            tick: 0,
            last_weather_tick: 0,
            roar_timer: 0,
            rain_timer: 0,
            shake: 0.0,
            instances: Vec::new(),
            received: 0,
        }
    }

    /// A message from the server half.
    pub fn on_message(&mut self, data: &[u8]) {
        self.received += 1;
        match net::kind_of(data) {
            Some(net::KIND_WEATHER) => {
                if let Some(m) = net::decode_weather(data, &mut self.env, &self.scale) {
                    if m.tick < self.last_weather_tick {
                        return;
                    }
                    self.last_weather_tick = m.tick;
                    if let Some(e) = &self.env {
                        if e.spec != self.weather.env.spec {
                            self.weather.env = e.clone();
                        }
                    }
                    self.weather.ground_y = m.ground_y;
                    self.weather.storms = m.storms;
                    self.weather.tornadoes = m.tornadoes;
                }
            }
            Some(net::KIND_DEBRIS) => {
                if let Some((_, pieces)) = net::decode_debris(data) {
                    // messages of one tick come in parts: replace by id
                    if self.debris_age > TICK * 1.5 {
                        self.debris.clear();
                    }
                    self.debris_age = 0.0;
                    for p in pieces {
                        if let Some(old) = self.debris.iter_mut().find(|d| d.id == p.id) {
                            *old = p;
                        } else {
                            self.debris.push(p);
                        }
                    }
                }
            }
            Some(net::KIND_EFFECTS) => {
                if let Some((_, fx)) = net::decode_effects(data) {
                    for f in fx {
                        self.effect(f);
                    }
                }
            }
            Some(net::KIND_TRACK) => {
                if let Some((id, pts)) = net::decode_track(data) {
                    self.tracks.insert(id, pts);
                }
            }
            _ => {}
        }
    }

    fn nearest_tornado_id(&self, at: Vec3) -> u32 {
        self.weather
            .tornadoes
            .iter()
            .filter(|t| (t.pos - at).horizontal_length() < t.influence_radius())
            .min_by(|a, b| (a.pos - at).horizontal_length().total_cmp(&(b.pos - at).horizontal_length()))
            .map_or(0, |t| t.id)
    }

    fn effect(&mut self, f: Fx) {
        let ear = self.player;
        match f {
            Fx::Burst { pos, vel, state } => {
                let tid = self.nearest_tornado_id(pos);
                emitters::burst(&mut self.particles, pos, vel, &state, tid);
                if self.settings.sound && self.sounds.len() < 6 {
                    self.sounds.extend(event(break_sound(&state), ear, pos, 0.7, 1.0));
                }
            }
            Fx::Fragment { pos, state, .. } => {
                emitters::burst(&mut self.particles, pos, Vec3::ZERO, &state, 0);
            }
            Fx::TreeFall { pos } => {
                if self.settings.sound {
                    self.sounds.extend(event("minecraft:entity.zombie.break_wooden_door", ear, pos, 0.9, 0.6));
                }
            }
            Fx::Collapse { pos, blocks } => {
                if self.settings.sound {
                    let loud = (0.4 + blocks as f64 / 60.0).min(1.0);
                    self.sounds.extend(event("minecraft:entity.generic.explode", ear, pos, loud, 0.55));
                }
                for _ in 0..(blocks.min(12)) {
                    emitters::burst(&mut self.particles, pos, Vec3::new(0.0, 2.0, 0.0), "minecraft:dirt", 0);
                }
            }
            Fx::Lightning { pos } => {
                let base = self.weather.storm_near(pos).map_or(120.0, |s| s.cloud_base);
                if (pos - ear).horizontal_length() < 4000.0 {
                    emitters::lightning(&mut self.particles, pos, base);
                }
                if self.settings.sound {
                    self.sounds.extend(thunder(ear, pos));
                }
            }
            Fx::Notice { text } => self.chat.push(text),
        }
    }

    /// One client tick (20 Hz) with the player at `player` and the world's time of day.
    pub fn tick(&mut self, player: Vec3, day_time: i64) {
        self.tick += 1;
        self.player = player;
        self.debris_age += TICK;
        // move the replica on between messages
        for s in &mut self.weather.storms {
            s.pos += s.motion * TICK;
        }
        for t in &mut self.weather.tornadoes {
            t.pos += t.velocity * TICK;
            let omega = 0.55 * t.params.v_max / t.params.r_max.max(5.0);
            for v in &mut t.multivortex.vortices {
                v.angle += omega * TICK;
                v.age += TICK;
            }
            t.wave_phase += TICK * 0.5;
        }
        for d in &mut self.debris {
            d.pos += d.vel * TICK;
            d.vel.y -= 9.8 * TICK * 0.5;
        }
        if self.debris_age > 1.0 {
            self.debris.clear();
        }
        // particles
        emitters::emit_all(&mut self.particles, &self.weather, player, TICK);
        self.particles.update(&self.weather, TICK, player);
        // subvortex tracks for the scientific view
        if self.settings.science {
            for t in &self.weather.tornadoes {
                for (i, v) in t.multivortex.vortices.iter().enumerate() {
                    let q = self.sub_tracks.entry((t.id, i)).or_default();
                    q.push_back(t.pos + v.offset());
                    if q.len() > 120 {
                        q.pop_front();
                    }
                }
            }
            let live: Vec<u32> = self.weather.tornadoes.iter().map(|t| t.id).collect();
            self.sub_tracks.retain(|k, _| live.contains(&k.0));
        }
        // radar
        if let Some(r) = &mut self.radar {
            r.tick(&self.weather, self.tick);
        }
        self.plan_sounds();
        // shake: the ground trembles near a strong tornado
        let mut shake: f32 = 0.0;
        for t in &self.weather.tornadoes {
            let d = (t.pos - player).horizontal_length();
            let r = t.radius_of_wind(29.0).max(30.0);
            let s = (1.0 - smoothstep(r * 0.5, r * 4.0, d)) * (t.params.v_max / 90.0).min(1.2);
            shake = shake.max(s as f32);
        }
        self.shake = shake;
        self.publish(day_time);
    }

    fn plan_sounds(&mut self) {
        if !self.settings.sound {
            return;
        }
        if self.roar_timer == 0 {
            if let Some(t) = self
                .weather
                .tornadoes
                .iter()
                .min_by(|a, b| (a.pos - self.player).horizontal_length().total_cmp(&(b.pos - self.player).horizontal_length()))
            {
                if let Some(c) = roar(self.player, t.pos, t.params.v_max, t.params.r_max) {
                    self.sounds.push(c);
                }
            }
            self.roar_timer = 50;
        } else {
            self.roar_timer -= 1;
        }
        if self.rain_timer == 0 {
            let rain: f64 = self.weather.storms.iter().map(|s| s.hydrometeors(self.player, 400.0).rain).sum();
            let hail: f64 = self.weather.storms.iter().map(|s| s.hydrometeors(self.player, 400.0).hail).sum();
            if rain > 0.2 {
                self.sounds.push(SoundCue { sound: "minecraft:weather.rain", at: self.player + Vec3::new(0.0, 4.0, 0.0), volume: (rain / 3.0).min(1.0) as f32, pitch: 0.9, delay: 0 });
            }
            if hail > 0.3 {
                self.sounds.push(SoundCue { sound: "minecraft:block.stone.hit", at: self.player + Vec3::new(1.0, 2.0, 0.0), volume: (hail / 2.0).min(1.0) as f32, pitch: 1.6, delay: 0 });
            }
            self.rain_timer = 30;
        } else {
            self.rain_timer -= 1;
        }
        self.sounds.truncate(12);
    }

    /// Ambient light for the particles: day and night, darker under a storm, lightning flashes.
    pub fn light(&self, day_time: i64) -> f32 {
        let a = (day_time.rem_euclid(24_000) as f64 / 24_000.0) * std::f64::consts::TAU;
        let sun = 0.16 + 0.84 * smoothstep(-0.15, 0.3, a.sin());
        let overhead: f64 = self.weather.storms.iter().map(|s| s.hydrometeors(self.player, 1500.0).total()).sum();
        let dark = 1.0 - 0.45 * (overhead / 3.0).min(1.0);
        (sun * dark) as f32 + self.particles.flash * 0.9
    }

    fn publish(&mut self, day_time: i64) {
        let anchor = Vec3::new(self.player.x.floor(), self.player.y.floor(), self.player.z.floor());
        let light = self.light(day_time);
        let mut out = std::mem::take(&mut self.instances);
        out.clear();
        self.particles.instances(self.player, anchor, 1.0, &mut out);
        // debris pieces the server is simulating
        for d in &self.debris {
            let c = block_color(&d.state);
            out.push(Instance {
                pos: [(d.pos.x - anchor.x) as f32, (d.pos.y - anchor.y) as f32, (d.pos.z - anchor.z) as f32],
                size: (d.size * 0.6) as f32,
                vel: [d.vel.x as f32, d.vel.y as f32, d.vel.z as f32],
                color: c,
                shape: Shape::Chunk as u32 | ((d.id & 255) << 8),
            });
        }
        if self.settings.science {
            self.science_lines(anchor, &mut out);
        }
        if self.show_track {
            self.track_lines(anchor, &mut out);
        }
        render_bridge::publish(Frame { anchor: anchor.to_array(), instances: out.clone(), built_at: render_bridge::now(), light, version: 0 });
        self.instances = out;
    }

    fn line(out: &mut Vec<Instance>, anchor: Vec3, a: Vec3, b: Vec3, width: f32, color: [u8; 4]) {
        let d = b - a;
        out.push(Instance {
            pos: [(a.x - anchor.x) as f32, (a.y - anchor.y) as f32, (a.z - anchor.z) as f32],
            size: width,
            vel: [d.x as f32, d.y as f32, d.z as f32],
            color,
            shape: Shape::Line as u32,
        });
    }

    fn ring(out: &mut Vec<Instance>, anchor: Vec3, c: Vec3, r: f64, y: f64, color: [u8; 4]) {
        if r <= 0.5 {
            return;
        }
        let n = 48;
        for k in 0..n {
            let a0 = k as f64 / n as f64 * std::f64::consts::TAU;
            let a1 = (k + 1) as f64 / n as f64 * std::f64::consts::TAU;
            let p0 = Vec3::new(c.x + r * a0.cos(), y, c.z + r * a0.sin());
            let p1 = Vec3::new(c.x + r * a1.cos(), y, c.z + r * a1.sin());
            Self::line(out, anchor, p0, p1, 0.25, color);
        }
    }

    /// The scientific overlay as lines in the world.
    fn science_lines(&self, anchor: Vec3, out: &mut Vec<Instance>) {
        // wind vectors on a grid around the player, two levels
        for &dy in &[2.0, 24.0] {
            for i in -4..=4 {
                for j in -4..=4 {
                    let p = Vec3::new(self.player.x + i as f64 * 8.0, self.player.y + dy, self.player.z + j as f64 * 8.0);
                    let w = self.weather.wind_at(p);
                    let s = w.length();
                    let col = speed_color(s);
                    Self::line(out, anchor, p, p + w * 0.15, 0.12, col);
                }
            }
        }
        for t in &self.weather.tornadoes {
            let g = t.pos.y + 0.6;
            // damaging-wind limits by EF lower bounds: 29, 49 and 74 m/s
            Self::ring(out, anchor, t.pos, t.radius_of_wind(29.0), g, [255, 170, 40, 220]);
            Self::ring(out, anchor, t.pos, t.radius_of_wind(49.0), g + 0.2, [255, 40, 40, 220]);
            Self::ring(out, anchor, t.pos, t.radius_of_wind(74.0), g + 0.4, [255, 40, 255, 220]);
            // isobars of the pressure deficit: 10, 20, 40 hPa
            for (dp, c) in [(1000.0, [80, 160, 255, 160]), (2000.0, [60, 110, 255, 180]), (4000.0, [40, 60, 255, 200])] {
                if let Some(r) = t.params.radius_of_deficit(dp, 1.15) {
                    Self::ring(out, anchor, t.pos, r, g + 1.0, c);
                }
            }
            // the funnel's axis and the condensation bottom
            let lv = &t.funnel.levels;
            for w in lv.windows(2) {
                Self::line(out, anchor, t.pos + w[0].offset + Vec3::new(0.0, w[0].height, 0.0), t.pos + w[1].offset + Vec3::new(0.0, w[1].height, 0.0), 0.3, [80, 255, 255, 200]);
            }
            let bottom = t.pos + Vec3::new(0.0, t.funnel.bottom, 0.0);
            Self::ring(out, anchor, bottom, 6.0, bottom.y, [80, 255, 255, 255]);
            // updraft and downdraft columns at the core
            let w = t.params.radial_vertical(0.0, 50.0).1;
            let col = if w >= 0.0 { [255, 120, 60, 200] } else { [60, 120, 255, 200] };
            Self::line(out, anchor, t.pos, t.pos + Vec3::new(0.0, w.abs() * 1.5, 0.0), 0.6, col);
        }
        for q in self.sub_tracks.values() {
            let pts: Vec<&Vec3> = q.iter().collect();
            for w in pts.windows(2) {
                Self::line(out, anchor, *w[0] + Vec3::new(0.0, 1.0, 0.0), *w[1] + Vec3::new(0.0, 1.0, 0.0), 0.2, [255, 255, 255, 200]);
            }
        }
        // circulations the radar detected
        if let Some(s) = self.radar.as_ref().and_then(|r| r.sweep(0, 0)) {
            for c in &s.couplets {
                let base = Vec3::new(c.pos.x, self.weather.ground_y, c.pos.z);
                let col = if c.tvs { [255, 30, 30, 255] } else { [255, 230, 60, 255] };
                Self::line(out, anchor, base, base + Vec3::new(0.0, 80.0, 0.0), 1.0, col);
            }
            for t in &s.tds {
                let base = Vec3::new(t.x, self.weather.ground_y, t.z);
                Self::line(out, anchor, base, base + Vec3::new(0.0, 100.0, 0.0), 1.5, [190, 60, 255, 255]);
            }
        }
    }

    /// Tornado paths coloured by intensity.
    fn track_lines(&self, anchor: Vec3, out: &mut Vec<Instance>) {
        let y = self.weather.ground_y + 1.5;
        for pts in self.tracks.values() {
            for w in pts.windows(2) {
                let a = Vec3::new(w[0].0, y, w[0].1);
                let b = Vec3::new(w[1].0, y, w[1].1);
                if (a - self.player).horizontal_length() > 2500.0 {
                    continue;
                }
                Self::line(out, anchor, a, b, 1.2, speed_color(w[1].2));
            }
        }
    }

    /// A client command; None when it is not ours.
    pub fn command(&mut self, line: &str) -> Option<Vec<String>> {
        let cmd = match parse_client(line)? {
            Ok(c) => c,
            Err(e) => return Some(vec![e]),
        };
        let mut out = Vec::new();
        match cmd {
            ClientCommand::Help => out.extend(CLIENT_HELP.iter().map(|s| s.to_string())),
            ClientCommand::RadarOpen => {
                self.ensure_radar();
                self.radar_open = true;
                out.push("Radar aberto. /radar product velocity|reflectivity|cc, /radar tilt n, /radar range km".into());
            }
            ClientCommand::RadarClose => self.radar_open = false,
            ClientCommand::RadarProduct(p) => {
                self.ensure_radar();
                self.radar_open = true;
                self.view.product = p;
            }
            ClientCommand::RadarTilt(i) => {
                let n = self.radar.as_ref().map_or(9, |r| r.site.vcp.len());
                self.view.tilt = i.min(n - 1);
                let e = self.radar.as_ref().map_or(0.5, |r| r.site.vcp[self.view.tilt]);
                out.push(format!("Elevação {e:.1}°"));
            }
            ClientCommand::RadarRange(km) => self.view.range = km * 1000.0,
            ClientCommand::RadarHere => {
                self.radar_site = Some(self.player + Vec3::new(0.0, 15.0, 0.0));
                self.radar = None;
                self.ensure_radar();
                out.push(format!("Radar instalado em ({:.0}, {:.0}, {:.0})", self.player.x, self.player.y, self.player.z));
            }
            ClientCommand::RadarExpand(on) => self.view.expanded = on,
            ClientCommand::RadarHistory(n) => {
                let have = self.radar.as_ref().map_or(0, |r| r.history.len());
                self.view.back = n.min(have.saturating_sub(1));
                out.push(format!("Histórico: {} varreduras guardadas; mostrando -{}", have, self.view.back));
            }
            ClientCommand::RadarAliasing(on) => {
                self.ensure_radar();
                if let Some(r) = &mut self.radar {
                    let mut site = r.site.clone();
                    site.aliasing = on;
                    r.set_site(site);
                }
                out.push(format!("Aliasing de velocidade {}", if on { "ligado (Nyquist 27 m/s)" } else { "desligado" }));
            }
            ClientCommand::RadarBand(b) => {
                self.ensure_radar();
                if let Some(r) = &mut self.radar {
                    let mut site = r.site.clone();
                    site.band = b;
                    r.set_site(site);
                }
                out.push(format!("Banda {}", b.name()));
            }
            ClientCommand::Quality(q) => {
                self.settings.quality = q;
                self.particles.set_quality(q);
                if let Some(r) = &mut self.radar {
                    let (radials, gates) = q.radar_resolution();
                    r.radials = radials;
                    r.gates = gates;
                    r.subsamples = q.radar_subsamples();
                }
                out.push(format!("Qualidade visual: {} (até {} partículas; a física dos danos não muda)", q.name(), q.particle_budget()));
            }
            ClientCommand::Science(on) => {
                self.settings.science = on;
                out.push(format!("Modo científico {}", if on { "ligado" } else { "desligado" }));
            }
            ClientCommand::Track(on) => {
                self.show_track = on;
                out.push(format!("Trajetórias {} ({} conhecidas)", if on { "visíveis" } else { "ocultas" }, self.tracks.len()));
            }
            ClientCommand::Shake(on) => self.settings.shake = on,
            ClientCommand::Sound(on) => self.settings.sound = on,
        }
        Some(out)
    }

    fn ensure_radar(&mut self) {
        if self.radar.is_some() {
            return;
        }
        let site = self.radar_site.unwrap_or(self.player + Vec3::new(0.0, 15.0, 0.0));
        let (radials, gates) = self.settings.quality.radar_resolution();
        let mut r = Radar::new(RadarSite::new(site, 1.0 / self.scale.storm_length), radials, gates, self.settings.quality.radar_subsamples());
        r.start_worker();
        self.radar = Some(r);
    }

    /// The radar panel's size, when open.
    pub fn radar_panel_size(&self) -> Option<(f32, f32)> {
        self.radar_open.then(|| self.view.panel_size())
    }

    /// The radar panel at (x, y).
    pub fn radar_panel(&self, x: f32, y: f32) -> Vec<HudOp> {
        let Some(r) = &self.radar else { return Vec::new() };
        let sweep = r.sweep(self.view.tilt, self.view.back);
        radar_panel(x, y, &self.view, sweep, r.site.band.name(), self.tick, self.player, r.site.pos, r.site.scale)
    }

    /// The tornado warning line, if a tornado is near and closing in.
    pub fn warning(&self) -> Option<String> {
        let t = self.weather.tornadoes.iter().min_by(|a, b| (a.pos - self.player).horizontal_length().total_cmp(&(b.pos - self.player).horizontal_length()))?;
        let d = t.pos - self.player;
        let dist = d.horizontal_length();
        if dist > 2500.0 {
            return None;
        }
        let closing = -(t.velocity.dot(d.horizontal().normalized()));
        let dir = {
            let deg = d.x.atan2(-d.z).to_degrees().rem_euclid(360.0);
            ["N", "NE", "E", "SE", "S", "SW", "W", "NW"][((deg + 22.5) / 45.0) as usize % 8]
        };
        let eta = if closing > 0.5 { format!(", chegada em ~{:.0} s", (dist - t.radius_of_wind(29.0)).max(0.0) / closing) } else { String::new() };
        Some(format!("AVISO DE TORNADO: {:.1} km a {dir}{eta}", dist / 1000.0))
    }

    /// Lines of the scientific readout.
    pub fn science_text(&self) -> Vec<String> {
        if !self.settings.science {
            return Vec::new();
        }
        let w = self.weather.wind_at(self.player + Vec3::new(0.0, 2.0, 0.0));
        let mut lines = vec![format!(
            "Vento: {:.1} m/s ({:.0} km/h), vertical {:+.1} m/s | partículas {} | detritos {}",
            w.horizontal_length(),
            ms_to_kmh(w.horizontal_length()),
            w.y,
            self.particles.len(),
            self.debris.len()
        )];
        for t in &self.weather.tornadoes {
            let d = (t.pos - self.player).horizontal_length();
            let dp = t.params.pressure_deficit(d, 1.15) / 100.0;
            lines.push(format!(
                "#{} {} [{}] {:.0} m: Vmax {:.0} m/s Rmax {:.0} m beta {:.1} | ΔP aqui {:.1} hPa | condensação desce a {:.0} m | subvórtices {}",
                t.id,
                t.morphology().name(),
                t.stage.name(),
                d,
                t.params.v_max,
                t.params.r_max,
                t.params.beta,
                dp,
                t.funnel.bottom,
                t.multivortex.vortices.len()
            ));
        }
        lines
    }

    /// Camera shake (pitch, yaw, roll degrees; offset blocks).
    pub fn shake(&self, time: f64) -> ([f32; 3], [f32; 3]) {
        if !self.settings.shake || self.shake <= 0.01 {
            return ([0.0; 3], [0.0; 3]);
        }
        let s = self.shake;
        let n = |k: f64| ((time * k).sin() * 0.6 + (time * k * 2.3).sin() * 0.4) as f32;
        ([n(13.0) * 0.8 * s, n(11.0) * 0.6 * s, n(7.0) * 0.5 * s], [n(17.0) * 0.04 * s, n(19.0) * 0.03 * s, 0.0])
    }

    /// Settings as text for storage, and back.
    pub fn settings_string(&self) -> String {
        format!(
            "quality={} science={} shake={} sound={}",
            self.settings.quality.name(),
            self.settings.science,
            self.settings.shake,
            self.settings.sound
        )
    }
    pub fn load_settings(&mut self, s: &str) {
        for kv in s.split_whitespace() {
            if let Some((k, v)) = kv.split_once('=') {
                match k {
                    "quality" => {
                        if let Some(q) = Quality::parse(v) {
                            self.settings.quality = q;
                            self.particles.set_quality(q);
                        }
                    }
                    "science" => self.settings.science = v == "true",
                    "shake" => self.settings.shake = v == "true",
                    "sound" => self.settings.sound = v == "true",
                    _ => {}
                }
            }
        }
    }

    /// The player left the world: forget everything.
    pub fn reset(&mut self) {
        self.weather.storms.clear();
        self.weather.tornadoes.clear();
        self.debris.clear();
        self.particles.clear();
        self.radar = None;
        self.radar_open = false;
        self.tracks.clear();
        self.last_weather_tick = 0;
        render_bridge::clear();
    }
}

/// Colours by wind speed along the EF lower bounds.
pub fn speed_color(v: f64) -> [u8; 4] {
    match v {
        v if v < 20.0 => [150, 200, 255, 200],
        v if v < 29.0 => [120, 255, 140, 220],
        v if v < 38.0 => [255, 255, 80, 230],
        v if v < 49.0 => [255, 170, 40, 240],
        v if v < 61.0 => [255, 70, 40, 250],
        v if v < 74.0 => [220, 20, 60, 255],
        v if v < 89.0 => [255, 40, 255, 255],
        _ => [255, 255, 255, 255],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage::world::testing::MemoryWorld;
    use crate::sim::{OVERWORLD, PlayerView, ServerSim};

    #[test]
    fn client_rebuilds_the_servers_weather_and_draws_it() {
        let mut server = ServerSim::new(3);
        let mut world = MemoryWorld::flat(64);
        let p = PlayerView { uuid: 1, name: "a".into(), pos: Vec3::new(0.0, 65.0, 0.0), dimension: OVERWORLD.into(), permission: 4 };
        server.command("storm env outbreak", &p);
        server.command("tornado spawn stovepipe", &p);
        let mut client = ClientState::new(ClientSettings::new());
        client.command("radar open");
        for _ in 0..200 {
            for m in server.tick(&mut world, std::slice::from_ref(&p), 6000) {
                client.on_message(&m.data);
            }
            client.tick(p.pos, 6000);
        }
        assert_eq!(client.weather.tornadoes.len(), 1);
        let (s, c) = (&server.weather.tornadoes[0], &client.weather.tornadoes[0]);
        assert!((s.pos - c.pos).horizontal_length() < 5.0, "the replica keeps up");
        assert!(client.particles.len() > 100);
        let f = render_bridge::latest(0).unwrap();
        assert!(!f.instances.is_empty());
        assert!(client.warning().is_some());
        assert!(client.radar_panel(0.0, 0.0).len() > 3);
        // settings survive a round trip
        client.command("sfx quality high");
        let saved = client.settings_string();
        let mut other = ClientState::new(ClientSettings::new());
        other.load_settings(&saved);
        assert_eq!(other.settings.quality, Quality::High);
    }
}
