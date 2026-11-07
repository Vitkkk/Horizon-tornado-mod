// Commands. Horizon keeps vanilla's `/weather`, so a mod cannot add `/weather storm ...`; storm
// commands live under `/storm` instead. `/tornado` and `/storm` run on the server (they change
// the world); `/radar` and `/sfx` run on the client (what it shows).

use crate::atmosphere::presets::Preset;
use crate::config::Quality;
use crate::radar::scanning::Product;
use crate::storms::StormKind;
use crate::tornado::SpawnShape;

#[derive(Clone, Debug, PartialEq)]
pub enum ServerCommand {
    /// `/storm <kind> [distance]`; `microburst` spawns a pulse storm about to collapse.
    Storm { kind: StormKind, microburst: bool, distance: Option<f64> },
    Env(Preset),
    Clear,
    Debug,
    Natural(bool),
    TimeScale(f64),
    /// Ask the nearest rotating storm for a tornado now.
    ForceTornado,
    TornadoSpawn(SpawnShape),
    Intensity(f64),
    Multivortex(bool),
    Speed(f64),
    Damage(bool),
    Survey(Option<u32>),
    List,
    Dissipate,
    Track(Option<u32>),
    Help,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClientCommand {
    RadarOpen,
    RadarClose,
    RadarProduct(Product),
    /// Elevation index of the volume pattern (0 lowest).
    RadarTilt(usize),
    /// Display half-width (km).
    RadarRange(f64),
    /// Move the radar site to where the player stands.
    RadarHere,
    RadarExpand(bool),
    /// Show the sweep `n` volumes back (0 live).
    RadarHistory(usize),
    RadarAliasing(bool),
    RadarBand(crate::radar::scanning::Band),
    Quality(Quality),
    Science(bool),
    Track(bool),
    Shake(bool),
    Sound(bool),
    Help,
}

/// The usages registered in the server's command tree (Horizon's `NativeCommand` syntax) and
/// the operator level each needs.
pub const SERVER_USAGES: &[(&str, u8)] = &[
    ("storm <kind:word> [distance:number]", 2),
    ("storm env <preset:word>", 2),
    ("storm clear", 2),
    ("storm debug", 0),
    ("storm natural <on:boolean>", 2),
    ("storm timescale <factor:number>", 2),
    ("storm tornado", 2),
    ("tornado spawn [shape:word]", 2),
    ("tornado intensity <speed:number>", 2),
    ("tornado multivortex <on:boolean>", 2),
    ("tornado speed <speed:number>", 2),
    ("tornado damage <state:word>", 2),
    ("tornado dissipate", 2),
    ("tornado survey [id:integer]", 0),
    ("tornado list", 0),
    ("tornado track [id:integer]", 0),
    ("tornado help", 0),
];

/// Client command names (without slash).
pub const CLIENT_NAMES: &[&str] = &["radar", "sfx"];

fn on_off(s: Option<&str>) -> Option<bool> {
    match s?.to_ascii_lowercase().as_str() {
        "on" | "true" | "1" | "yes" | "sim" | "ligado" => Some(true),
        "off" | "false" | "0" | "no" | "nao" | "não" | "desligado" => Some(false),
        _ => None,
    }
}

fn number(s: Option<&str>) -> Option<f64> {
    let v: f64 = s?.trim_end_matches("m/s").trim_end_matches("ms").parse().ok()?;
    v.is_finite().then_some(v)
}

/// Parse a server command line (without the slash). `Err` holds a message for the player.
pub fn parse_server(line: &str) -> Option<Result<ServerCommand, String>> {
    let mut it = line.split_whitespace();
    let root = it.next()?;
    let a = it.next();
    let b = it.next();
    let c = it.next();
    let _ = c;
    Some(match root {
        "storm" => match a {
            None | Some("help") => Ok(ServerCommand::Help),
            Some("env") => match b.and_then(Preset::parse) {
                Some(p) => Ok(ServerCommand::Env(p)),
                None => Err("presets: classic, outbreak, hp, lp, qlcs, pulse, microburst, marginal".into()),
            },
            Some("clear") => Ok(ServerCommand::Clear),
            Some("debug") => Ok(ServerCommand::Debug),
            Some("natural") => on_off(b).map(ServerCommand::Natural).ok_or_else(|| "use: /storm natural on|off".into()),
            Some("timescale") => match number(b) {
                Some(v) if (0.25..=60.0).contains(&v) => Ok(ServerCommand::TimeScale(v)),
                _ => Err("use: /storm timescale <0.25..60>".into()),
            },
            Some("tornado") => Ok(ServerCommand::ForceTornado),
            Some("microburst") | Some("downburst") => Ok(ServerCommand::Storm { kind: StormKind::Single, microburst: true, distance: number(b) }),
            Some(k) => match StormKind::parse(k) {
                Some(kind) => Ok(ServerCommand::Storm { kind, microburst: false, distance: number(b) }),
                None => Err(format!("unknown storm kind '{k}': {}", StormKind::NAMES)),
            },
        },
        "tornado" => match a {
            None | Some("help") => Ok(ServerCommand::Help),
            Some("spawn") => match b {
                None => Ok(ServerCommand::TornadoSpawn(SpawnShape::Cone)),
                Some(s) => SpawnShape::parse(s).map(ServerCommand::TornadoSpawn).ok_or_else(|| format!("shapes: {}", SpawnShape::NAMES)),
            },
            Some("intensity") => match number(b) {
                Some(v) if (10.0..=140.0).contains(&v) => Ok(ServerCommand::Intensity(v)),
                _ => Err("use: /tornado intensity <10..140 m/s>".into()),
            },
            Some("multivortex") => on_off(b).map(ServerCommand::Multivortex).ok_or_else(|| "use: /tornado multivortex true|false".into()),
            Some("speed") => match number(b) {
                Some(v) if (0.0..=40.0).contains(&v) => Ok(ServerCommand::Speed(v)),
                _ => Err("use: /tornado speed <0..40 m/s>".into()),
            },
            Some("damage") => on_off(b).map(ServerCommand::Damage).ok_or_else(|| "use: /tornado damage on|off".into()),
            Some("survey") => Ok(ServerCommand::Survey(b.and_then(|s| s.parse().ok()))),
            Some("list") => Ok(ServerCommand::List),
            Some("dissipate") => Ok(ServerCommand::Dissipate),
            Some("track") => Ok(ServerCommand::Track(b.and_then(|s| s.parse().ok()))),
            Some(other) => Err(format!("unknown: /tornado {other}; try /tornado help")),
        },
        _ => return None,
    })
}

/// Parse a client command line (without the slash).
pub fn parse_client(line: &str) -> Option<Result<ClientCommand, String>> {
    let mut it = line.split_whitespace();
    let root = it.next()?;
    let a = it.next();
    let b = it.next();
    Some(match root {
        "radar" => match a {
            None | Some("open") => Ok(ClientCommand::RadarOpen),
            Some("close") => Ok(ClientCommand::RadarClose),
            Some("help") => Ok(ClientCommand::Help),
            Some("product") => b.and_then(Product::parse).map(ClientCommand::RadarProduct).ok_or_else(|| "use: /radar product reflectivity|velocity|cc".into()),
            Some("reflectivity" | "ref" | "velocity" | "vel" | "cc") => Ok(ClientCommand::RadarProduct(Product::parse(a.unwrap()).unwrap())),
            Some("tilt") => b.and_then(|s| s.parse().ok()).map(ClientCommand::RadarTilt).ok_or_else(|| "use: /radar tilt <0..8>".into()),
            Some("range") => match number(b) {
                Some(v) if (1.0..=100.0).contains(&v) => Ok(ClientCommand::RadarRange(v)),
                _ => Err("use: /radar range <1..100 km>".into()),
            },
            Some("here" | "place") => Ok(ClientCommand::RadarHere),
            Some("expand") => Ok(ClientCommand::RadarExpand(on_off(b).unwrap_or(true))),
            Some("history" | "loop") => Ok(ClientCommand::RadarHistory(b.and_then(|s| s.parse().ok()).unwrap_or(0))),
            Some("aliasing") => on_off(b).map(ClientCommand::RadarAliasing).ok_or_else(|| "use: /radar aliasing on|off".into()),
            Some("band") => match b.map(|s| s.to_ascii_uppercase()) {
                Some(s) if s == "S" => Ok(ClientCommand::RadarBand(crate::radar::scanning::Band::S)),
                Some(s) if s == "C" => Ok(ClientCommand::RadarBand(crate::radar::scanning::Band::C)),
                Some(s) if s == "X" => Ok(ClientCommand::RadarBand(crate::radar::scanning::Band::X)),
                _ => Err("use: /radar band S|C|X".into()),
            },
            Some(other) => Err(format!("unknown: /radar {other}; try /radar help")),
        },
        "sfx" => match a {
            None | Some("help") => Ok(ClientCommand::Help),
            Some("quality") => b.and_then(Quality::parse).map(ClientCommand::Quality).ok_or_else(|| "use: /sfx quality low|medium|high".into()),
            Some("science" | "debug") => Ok(ClientCommand::Science(on_off(b).unwrap_or(true))),
            Some("track") => Ok(ClientCommand::Track(on_off(b).unwrap_or(true))),
            Some("shake") => on_off(b).map(ClientCommand::Shake).ok_or_else(|| "use: /sfx shake on|off".into()),
            Some("sound") => on_off(b).map(ClientCommand::Sound).ok_or_else(|| "use: /sfx sound on|off".into()),
            Some(other) => Err(format!("unknown: /sfx {other}")),
        },
        _ => return None,
    })
}

pub const SERVER_HELP: &[&str] = &[
    "Stormfront - comandos do servidor:",
    "/storm <supercell|hp|lp|multicell|single|qlcs|microburst> [distância]  - forma uma tempestade a montante",
    "/storm env <classic|outbreak|hp|lp|qlcs|pulse|microburst|marginal>  - muda a sondagem do ambiente",
    "/storm debug | clear | tornado | natural on|off | timescale <x>",
    "/tornado spawn <rope|cone|stovepipe|wedge|elephant|rainwrapped|multivortex|satellite>",
    "/tornado intensity <m/s> | speed <m/s> | multivortex true|false | damage on|off | dissipate",
    "/tornado list | survey [id] | track [id]",
];

pub const CLIENT_HELP: &[&str] = &[
    "Stormfront - comandos do cliente:",
    "/radar open|close | product reflectivity|velocity|cc | tilt <n> | range <km>",
    "/radar here | expand [on|off] | history <n> | aliasing on|off | band S|C|X",
    "/sfx quality low|medium|high | science on|off | track on|off | shake on|off | sound on|off",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_proposed_commands() {
        assert_eq!(parse_server("storm supercell"), Some(Ok(ServerCommand::Storm { kind: StormKind::Supercell(crate::storms::SupercellType::Classic), microburst: false, distance: None })));
        assert!(matches!(parse_server("storm hp 3000"), Some(Ok(ServerCommand::Storm { distance: Some(d), .. })) if d == 3000.0));
        assert_eq!(parse_server("tornado spawn cone"), Some(Ok(ServerCommand::TornadoSpawn(SpawnShape::Cone))));
        assert_eq!(parse_server("tornado spawn wedge"), Some(Ok(ServerCommand::TornadoSpawn(SpawnShape::Wedge))));
        assert_eq!(parse_server("tornado intensity 85"), Some(Ok(ServerCommand::Intensity(85.0))));
        assert_eq!(parse_server("tornado multivortex true"), Some(Ok(ServerCommand::Multivortex(true))));
        assert_eq!(parse_server("tornado speed 12"), Some(Ok(ServerCommand::Speed(12.0))));
        assert_eq!(parse_server("tornado damage on"), Some(Ok(ServerCommand::Damage(true))));
        assert_eq!(parse_server("tornado survey"), Some(Ok(ServerCommand::Survey(None))));
        assert_eq!(parse_server("storm debug"), Some(Ok(ServerCommand::Debug)));
        assert_eq!(parse_client("radar open"), Some(Ok(ClientCommand::RadarOpen)));
        assert_eq!(parse_client("radar product velocity"), Some(Ok(ClientCommand::RadarProduct(Product::Velocity))));
        assert_eq!(parse_client("radar product reflectivity"), Some(Ok(ClientCommand::RadarProduct(Product::Reflectivity))));
        assert_eq!(parse_client("radar product cc"), Some(Ok(ClientCommand::RadarProduct(Product::Correlation))));
        assert!(parse_server("give @p dirt").is_none());
        assert!(matches!(parse_server("tornado intensity 900"), Some(Err(_))));
    }

    #[test]
    fn usages_cover_every_root() {
        for (u, _) in SERVER_USAGES {
            let root = u.split(' ').next().unwrap();
            assert!(root == "storm" || root == "tornado");
        }
    }
}
