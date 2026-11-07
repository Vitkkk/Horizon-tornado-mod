// Stormfront's glue to Horizon, added to the game as `game::stormfront_glue`
// (crates/horizon-client/src/game.rs). It binds the simulation crate (`stormfront`, reached
// through `horizon_client_render::stormfront_render::engine`) to Horizon's hook points and
// helpers, all of them read from the pinned Horizon source:
//
// server half (authoritative; single player's server thread or a dedicated server):
//   level_tick_post   -> the weather, damage and records of the Overworld, then messages
//   register_commands -> /storm and /tornado in every player's command tree
//   perform_command   -> runs them, answering with native_feedback
//   world_lifecycle   -> a fresh simulation for each world
//   uses native_read_columns, native_edit_blocks, native_server_players,
//        native_net_send_to_player, native_config_dir
//
// client half (the window):
//   net_from_server   -> the server's weather, debris and effects
//   tick              -> particles, radar, sounds (native_play_local_sound), chat lines
//   hud               -> the radar panel, tornado warning and scientific readout
//   commands/command  -> /radar and /sfx
//   camera_shake      -> the ground trembling near a tornado
//   client_level_unload, mods
//
// Module payloads cannot carry inner attributes or `mod` files; plain comments only.

use super::Game;
use super::native_hooks::{NativeMod, NativeShake};
use super::native_server::{NativeCommand, NativeServerPlayer};
use super::native_world_edit::{NativeBlockEdit, NativeBlockRun, NativeHeightmap};
use horizon_client::extras::Corner;
use horizon_client::extras_hud::Corners;
use horizon_client::hud::HudBatch;
use horizon_client::mod_api::WorldPhase;
use horizon_client::text::Text;
use horizon_client_render::stormfront_render::engine as sf;
use sf::damage::world::{ColumnData, WorldIo};
use sf::hud::HudOp;
use sf::math::Vec3;
use sf::sim::{OVERWORLD, PlayerView, ServerSim};
use std::cell::RefCell;

pub(super) const ID: &str = "stormfront";
const NAME: &str = "Stormfront";
const VERSION: &str = "0.1.0";
const DESCRIPTION: &str = "Supercells, tornadoes with physical wind fields and damage, debris, ground scouring and a Doppler radar. /storm, /tornado, /radar, /sfx.";

thread_local! {
    // The server half lives on the thread that ticks the world, the client half on the window's.
    static SERVER: RefCell<Option<ServerSim>> = const { RefCell::new(None) };
    static CLIENT: RefCell<Option<sf::client::ClientState>> = const { RefCell::new(None) };
}

fn seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0x5703, |d| d.as_nanos() as u64)
}

// ---- the world, as the damage engine sees it --------------------------------------------------

struct HorizonIo<'a, 'g> {
    game: &'a mut Game<'g>,
    dimension: &'a str,
}

impl WorldIo for HorizonIo<'_, '_> {
    fn read_columns(&mut self, columns: &[(i32, i32)], below: u32, above: u32) -> Result<ColumnData, String> {
        let read = self.game.native_read_columns(self.dimension, columns, NativeHeightmap::MotionBlockingNoLeaves, (below, above))?;
        Ok(ColumnData { palette: read.palette, bases: read.bases, blocks: read.blocks })
    }

    fn edit_blocks(&mut self, edits: &[([i32; 3], String)]) -> Result<u64, String> {
        if edits.is_empty() {
            return Ok(0);
        }
        // one run per vertical stretch of equal states in a column
        let mut sorted: Vec<&([i32; 3], String)> = edits.iter().collect();
        sorted.sort_by_key(|(p, _)| (p[0], p[2], p[1]));
        let mut palette: Vec<String> = Vec::new();
        let mut runs: Vec<NativeBlockRun> = Vec::new();
        for (p, state) in sorted {
            let block = match palette.iter().position(|s| s == state) {
                Some(i) => i as u32,
                None => {
                    palette.push(state.clone());
                    (palette.len() - 1) as u32
                }
            };
            if let Some(last) = runs.last_mut() {
                if last.x == p[0] && last.z == p[2] && last.block == block && last.y + last.height as i32 == p[1] {
                    last.height += 1;
                    continue;
                }
            }
            runs.push(NativeBlockRun { x: p[0], z: p[2], y: p[1], height: 1, surface: None, block, only: None });
        }
        self.game.native_edit_blocks(self.dimension, &NativeBlockEdit { palette, filters: Vec::new(), runs })
    }
}

fn view_of(p: &NativeServerPlayer) -> PlayerView {
    PlayerView {
        uuid: p.uuid,
        name: p.name.clone(),
        pos: Vec3::new(p.position[0], p.position[1], p.position[2]),
        dimension: p.dimension.clone(),
        permission: p.permission,
    }
}

// ---- server half ----------------------------------------------------------------------------

/// `level_tick_post`: the Overworld's weather and damage, once a server tick.
pub(super) fn level_tick(game: &mut Game<'_>, dimension: &str) {
    if game.native_window() || !game.native_on(ID) || dimension != OVERWORLD {
        return;
    }
    let players: Vec<PlayerView> = game.native_server_players().iter().filter(|p| p.dimension == dimension).map(view_of).collect();
    let (out, files) = SERVER.with(|s| {
        let mut s = s.borrow_mut();
        let sim = s.get_or_insert_with(|| ServerSim::new(seed()));
        let mut io = HorizonIo { game: &mut *game, dimension };
        let out = sim.tick(&mut io, &players, 0);
        (out, std::mem::take(&mut sim.files))
    });
    for m in out {
        if let Err(e) = game.native_net_send_to_player(m.to, sf::net::CHANNEL, &m.data) {
            log::debug!("[mod {ID}] a message waits: {e}");
        }
    }
    write_files(files);
}

/// Surveys and tracks go to `config/stormfront/`.
fn write_files(files: Vec<(String, String)>) {
    if files.is_empty() {
        return;
    }
    let Ok(dir) = Game::native_config_dir() else { return };
    let dir = dir.join(ID);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    for (name, contents) in files {
        // names are the engine's own (`tornado-3.txt`); keep them plain anyway
        if name.contains('/') || name.contains('\\') || name.contains("..") {
            continue;
        }
        if let Err(e) = std::fs::write(dir.join(&name), contents) {
            log::warn!("[mod {ID}] could not write {name}: {e}");
        }
    }
}

/// `register_commands`: /storm and /tornado for every player.
pub(super) fn register_commands(commands: &mut Vec<NativeCommand>) {
    for &(usage, level) in sf::commands::SERVER_USAGES {
        commands.push(if level == 0 { NativeCommand::new(usage) } else { NativeCommand::op(usage, level) });
    }
}

/// `perform_command`: a player's /storm or /tornado.
pub(super) fn perform_command(game: &mut Game<'_>, player: &NativeServerPlayer, command: &str, ran: &mut bool) {
    if !game.native_on(ID) {
        return;
    }
    let root = command.split(' ').next().unwrap_or("");
    if root != "storm" && root != "tornado" {
        return;
    }
    let view = view_of(player);
    let (outcome, files) = SERVER.with(|s| {
        let mut s = s.borrow_mut();
        let sim = s.get_or_insert_with(|| ServerSim::new(seed()));
        let o = sim.command(command, &view);
        (o, std::mem::take(&mut sim.files))
    });
    if let Some(o) = outcome {
        *ran = true;
        for line in o.lines {
            game.native_feedback(Text::literal(line));
        }
    }
    write_files(files);
}

/// `world_lifecycle`: each world starts with clear skies.
pub(super) fn lifecycle(phase: WorldPhase) {
    if matches!(phase, WorldPhase::WorldOpenReady | WorldPhase::WorldCloseFinal | WorldPhase::WorldCloseFailed) {
        SERVER.with(|s| *s.borrow_mut() = None);
    }
}

// ---- client half ----------------------------------------------------------------------------

fn with_client<R>(game: &Game<'_>, f: impl FnOnce(&mut sf::client::ClientState) -> R) -> R {
    CLIENT.with(|c| {
        let mut c = c.borrow_mut();
        let state = c.get_or_insert_with(|| {
            let mut s = sf::client::ClientState::new(sf::config::ClientSettings::new());
            if let Some(saved) = game.native_setting(ID, "settings") {
                s.load_settings(&saved);
            }
            s
        });
        f(state)
    })
}

/// `net_from_server`: the server half's messages.
pub(super) fn from_server(game: &mut Game<'_>, channel: &str, data: &[u8]) {
    if channel != sf::net::CHANNEL || !game.native_window() || !game.native_on(ID) {
        return;
    }
    with_client(game, |c| c.on_message(data));
}

/// `tick`: the client half's step.
pub(super) fn tick(game: &mut Game<'_>) {
    if !game.native_window() {
        return;
    }
    if !game.native_on(ID) {
        sf::render_bridge::clear();
        return;
    }
    let Some(p) = game.native_player() else { return };
    if p.dimension != OVERWORLD {
        sf::render_bridge::clear();
        return;
    }
    let day = game.native_clock().map_or(6000, |c| c.day_time);
    let eye = Vec3::new(p.x, p.y + 1.62, p.z);
    let (sounds, chat) = with_client(game, |c| {
        c.tick(eye, day);
        (std::mem::take(&mut c.sounds), std::mem::take(&mut c.chat))
    });
    for s in sounds {
        game.native_play_local_sound(s.sound, s.at.to_array(), s.volume, s.pitch, s.delay);
    }
    for line in chat {
        game.native_chat(&line);
    }
}

fn replay(game: &Game<'_>, batch: &mut HudBatch, ops: &[HudOp]) {
    for op in ops {
        match op {
            HudOp::Fill { rect, argb } => game.native_fill(batch, *rect, *argb),
            HudOp::Text { at, text, rgb, shadow } => game.native_text(batch, text, *at, *rgb, *shadow),
        }
    }
}

/// `hud`: the radar panel, the warning banner and the scientific readout.
pub(super) fn hud(game: &mut Game<'_>, batch: &mut HudBatch, corners: &mut Corners) {
    if !game.native_on(ID) || game.native_player().is_none() {
        return;
    }
    let (warning, panel, science) = with_client(game, |c| (c.warning(), c.radar_panel_size(), c.science_text()));
    if let Some(w) = warning {
        let tw = game.native_text_width(&w);
        let x = ((batch.width - tw) / 2.0).max(2.0);
        game.native_fill(batch, [x - 4.0, 16.0, tw + 8.0, 13.0], 0xD070_0000);
        game.native_text(batch, &w, [x, 18.0], 0xFFFFFF, true);
    }
    if let Some((w, h)) = panel {
        if game.native_corner_shown(Corner::TopRight) {
            let (x, y) = corners.place(Corner::TopRight, w, h);
            let ops = with_client(game, |c| c.radar_panel(x, y));
            replay(game, batch, &ops);
        }
    }
    if !science.is_empty() && game.native_corner_shown(Corner::BottomLeft) {
        let w = science.iter().map(|l| game.native_text_width(l)).fold(0.0, f32::max) + 6.0;
        let h = 10.0 * science.len() as f32 + 4.0;
        let (x, y) = corners.place(Corner::BottomLeft, w, h);
        game.native_fill(batch, [x, y, w, h], sf::hud::BACKGROUND);
        for (i, line) in science.iter().enumerate() {
            game.native_text(batch, line, [x + 3.0, y + 3.0 + 10.0 * i as f32], sf::hud::INK, true);
        }
    }
}

/// `commands`: the client commands the chat box should know.
pub(super) fn commands(commands: &mut Vec<&'static str>) {
    commands.extend_from_slice(sf::commands::CLIENT_NAMES);
}

/// `command`: a client command (/radar, /sfx).
pub(super) fn command(game: &mut Game<'_>, command: &str, ran: &mut bool) {
    if !game.native_on(ID) {
        return;
    }
    let root = command.split(' ').next().unwrap_or("");
    if !sf::commands::CLIENT_NAMES.contains(&root) {
        return;
    }
    if let Some(p) = game.native_player() {
        with_client(game, |c| c.player = Vec3::new(p.x, p.y + 1.62, p.z));
    }
    let (lines, settings) = with_client(game, |c| (c.command(command), c.settings_string()));
    if let Some(lines) = lines {
        *ran = true;
        for l in lines {
            game.native_chat(&l);
        }
        game.native_set_setting(ID, "settings", &settings);
    }
}

/// `camera_shake`: near a strong tornado the view trembles.
pub(super) fn camera_shake(game: &Game<'_>, _partial: f32, shake: &mut NativeShake) {
    if !game.native_on(ID) {
        return;
    }
    let t = sf::render_bridge::now();
    let s = CLIENT.with(|c| c.borrow().as_ref().map(|c| c.shake(t)));
    if let Some(([pitch, yaw, roll], off)) = s {
        shake.pitch += pitch;
        shake.yaw += yaw;
        shake.roll += roll;
        for i in 0..3 {
            shake.offset[i] += off[i];
        }
    }
}

/// `client_level_unload`: leaving a level forgets what the client half held.
pub(super) fn level_unload() {
    CLIENT.with(|c| {
        if let Some(c) = c.borrow_mut().as_mut() {
            c.reset();
        }
    });
    sf::render_bridge::clear();
}

/// `mods`: the row in Options > Mods.
pub(super) fn listed(mods: &mut Vec<NativeMod>) {
    mods.push(NativeMod { id: ID.into(), title: format!("{NAME} {VERSION}"), description: DESCRIPTION.into(), keys: Vec::new() });
}
