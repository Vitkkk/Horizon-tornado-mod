// `stormfront-lite-tools pack <core.wasm> <mod.wasm>`: turns the wasm32 cdylib into the
// component Horizon loads (as Horizon's own `componentize` does, with the same wit-component)
// and prints the interfaces it imports, which the manifest's capabilities must cover.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 || args[1] != "pack" {
        eprintln!("usage: stormfront-lite-tools pack <core.wasm> <mod.wasm>");
        return ExitCode::from(2);
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
    for (key, item) in &resolve.worlds[world].imports {
        let name = match item {
            wit_parser::WorldItem::Interface { id, .. } => resolve.id_of(*id).unwrap_or_else(|| resolve.name_world_key(key)),
            _ => resolve.name_world_key(key),
        };
        println!("import {name}");
    }
    for (key, _) in &resolve.worlds[world].exports {
        println!("export {}", resolve.name_world_key(key));
    }
    if let Err(e) = std::fs::write(&args[3], &component) {
        eprintln!("{}: {e}", args[3]);
        return ExitCode::FAILURE;
    }
    eprintln!("{} bytes", component.len());
    ExitCode::SUCCESS
}
