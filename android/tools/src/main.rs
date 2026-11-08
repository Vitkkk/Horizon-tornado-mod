// `stormfront-lite-tools pack <core.wasm> <mod.wasm> <horizon-mod.toml>`: turns the wasm32
// cdylib into the component Horizon loads (as Horizon's own `componentize` does, with the same
// wit-component), then checks the manifest and the component with the rules Horizon Android
// applies when it imports and starts a mod (horizon-mods/src/manifest.rs and api_level.rs in
// that port): unknown keys, id, API line, side, client/server capabilities, the opt-in for
// experimental capabilities, imports covered by capabilities, and the API level of every
// imported function.

use serde::Deserialize;
use std::process::ExitCode;

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
enum Capability {
    GameRead,
    Chat,
    Hud,
    Storage,
    Command,
    Registries,
    Ui,
    Render,
    Input,
    Players,
    World,
    Recipes,
}

impl Capability {
    fn interface(self) -> &'static str {
        match self {
            Capability::GameRead => "game",
            Capability::Chat => "chat",
            Capability::Hud => "hud",
            Capability::Storage => "storage",
            Capability::Command => "command",
            Capability::Registries => "",
            Capability::Ui => "ui",
            Capability::Render => "render",
            Capability::Input => "input",
            Capability::Players => "players",
            Capability::World => "world",
            Capability::Recipes => "recipes",
        }
    }
    fn on_server(self) -> bool {
        matches!(self, Capability::Storage | Capability::Players | Capability::World)
    }
    fn experimental(self) -> bool {
        matches!(self, Capability::Ui | Capability::Render | Capability::Input | Capability::Recipes)
    }
}

#[derive(Deserialize, Default, PartialEq, Eq, Debug, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Side {
    #[default]
    Client,
    Server,
    Both,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct Manifest {
    id: String,
    name: String,
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    authors: Vec<String>,
    api: String,
    #[serde(default)]
    side: Side,
    #[serde(default)]
    capabilities: Vec<Capability>,
    #[serde(default)]
    namespaces: Vec<String>,
    #[serde(default)]
    experimental: Vec<Capability>,
}

/// Horizon Android 0.7.8's API level (api_level.rs `LEVEL`) and each function's level.
const LEVEL: u32 = 7;
const FUNCTIONS: [(&str, u32); 37] = [
    ("log.write", 0),
    ("game.get-player", 0),
    ("game.get-clock", 0),
    ("game.raycast", 2),
    ("chat.show", 0),
    ("hud.set-panel", 0),
    ("hud.clear", 0),
    ("storage.get", 0),
    ("storage.set", 0),
    ("storage.delete", 0),
    ("command.run", 0),
    ("command.run-quiet", 3),
    ("ui.open-screen", 0),
    ("ui.close-screen", 0),
    ("ui.set-settings", 0),
    ("input.claim-items", 0),
    ("input.register-key", 0),
    ("input.held", 0),
    ("input.poll", 0),
    ("input.claim-wheel", 0),
    ("input.mapping", 0),
    ("input.clicks-per-second", 0),
    ("render.set-hud", 0),
    ("render.gui-size", 0),
    ("render.shake", 0),
    ("render.set-corner", 0),
    ("render.clear-corner", 0),
    ("render.text-width", 0),
    ("render.frame-stats", 0),
    ("render.set-zoom", 0),
    ("render.set-minimap", 1),
    ("players.online", 1),
    ("players.tell", 1),
    ("players.broadcast", 1),
    ("world.get-block", 4),
    ("world.get-biome", 4),
    ("world.set-blocks", 4),
];

fn check_manifest(text: &str) -> Result<Manifest, String> {
    let m: Manifest = toml::from_str(text).map_err(|e| format!("horizon-mod.toml: {e}"))?;
    if !(1..=64).contains(&m.id.len()) || !m.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
        return Err(format!("id {:?} is not [a-z0-9_]{{1,64}}", m.id));
    }
    if !["0.1", "0.2", "0.3"].contains(&m.api.as_str()) {
        return Err(format!("api {} is not one Horizon Android runs (0.1, 0.2, 0.3)", m.api));
    }
    if m.capabilities.contains(&Capability::Registries) && m.api == "0.1" {
        return Err("capability registries needs api = \"0.2\"".into());
    }
    let server = m.side == Side::Server;
    if server != (m.api == "0.3") {
        return Err("a server mod is side = \"server\" with api = \"0.3\", and only it".into());
    }
    if let Some(c) = m.capabilities.iter().find(|c| c.on_server() != server && **c != Capability::Storage) {
        return Err(format!("capability {c:?} is for {} mods", if server { "client" } else { "server" }));
    }
    if let Some(c) = m.capabilities.iter().find(|c| c.experimental() && !m.experimental.contains(c)) {
        return Err(format!("capability {c:?} is experimental: add it to experimental = [...]"));
    }
    if !m.namespaces.is_empty() && !m.capabilities.contains(&Capability::Registries) {
        return Err("namespaces need capability registries".into());
    }
    Ok(m)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 5 || args[1] != "pack" {
        eprintln!("usage: stormfront-lite-tools pack <core.wasm> <mod.wasm> <horizon-mod.toml>");
        return ExitCode::from(2);
    }
    let manifest = match std::fs::read_to_string(&args[4]).map_err(|e| e.to_string()).and_then(|t| check_manifest(&t)) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("manifest refused: {e}");
            return ExitCode::FAILURE;
        }
    };
    if std::fs::metadata(&args[2]).map(|m| m.len()).unwrap_or(0) > 32 << 20 {
        eprintln!("mod.wasm over 32 MiB");
        return ExitCode::FAILURE;
    }
    let core = match std::fs::read(&args[2]) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{}: {e}", args[2]);
            return ExitCode::FAILURE;
        }
    };
    let component = match wit_component::ComponentEncoder::default().module(&core).and_then(|e| e.validate(true).encode()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("componentize: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    let wit_component::DecodedWasm::Component(resolve, world) = (match wit_component::decode(&component) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("decode: {e:#}");
            return ExitCode::FAILURE;
        }
    }) else {
        eprintln!("not a component");
        return ExitCode::FAILURE;
    };
    let mut refused = Vec::new();
    for (key, item) in &resolve.worlds[world].imports {
        let name = match item {
            wit_parser::WorldItem::Interface { id, .. } => resolve.id_of(*id).unwrap_or_else(|| resolve.name_world_key(key)),
            _ => resolve.name_world_key(key),
        };
        println!("import {name}");
        let Some(rest) = name.strip_prefix("horizon:mods/") else {
            refused.push(format!("imports {name}, which Horizon does not offer"));
            continue;
        };
        let (interface, version) = rest.split_once('@').unwrap_or((rest, ""));
        if !version.starts_with("0.1.") {
            refused.push(format!("{name} needs API {version}"));
        }
        if interface != "log" {
            let cap = if interface == "browser" { Some(Capability::Ui) } else { [Capability::GameRead, Capability::Chat, Capability::Hud, Capability::Storage, Capability::Command, Capability::Ui, Capability::Render, Capability::Input, Capability::Players, Capability::World, Capability::Recipes].into_iter().find(|c| c.interface() == interface) };
            match cap {
                Some(c) if manifest.capabilities.contains(&c) => {}
                Some(c) => refused.push(format!("uses capability {c:?}, missing from horizon-mod.toml")),
                None => refused.push(format!("imports {name}, which Horizon does not offer")),
            }
        }
        // the level of each function it imports
        if let wit_parser::WorldItem::Interface { id, .. } = item {
            for f in resolve.interfaces[*id].functions.keys() {
                let full = format!("{interface}.{f}");
                match FUNCTIONS.iter().find(|(n, _)| *n == full) {
                    Some((_, l)) if *l <= LEVEL => {}
                    Some((_, l)) => refused.push(format!("{full} needs API level {l} (this Horizon: {LEVEL})")),
                    None if interface == "recipes" || interface == "browser" => {}
                    None => refused.push(format!("{full} is not a function Horizon offers")),
                }
            }
        }
    }
    for (key, _) in &resolve.worlds[world].exports {
        println!("export {}", resolve.name_world_key(key));
    }
    if !refused.is_empty() {
        for r in refused {
            eprintln!("refused: {r}");
        }
        return ExitCode::FAILURE;
    }
    println!("manifest and component pass Horizon Android's checks ({} {}, api {}, capabilities {:?}, experimental {:?})", manifest.id, manifest.version, manifest.api, manifest.capabilities, manifest.experimental);
    if let Err(e) = std::fs::write(&args[3], &component) {
        eprintln!("{}: {e}", args[3]);
        return ExitCode::FAILURE;
    }
    eprintln!("{} bytes", component.len());
    ExitCode::SUCCESS
}
