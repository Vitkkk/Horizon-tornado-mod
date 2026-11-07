// The game's side of Stormfront Lite: WIT bindings for Horizon's `horizon-ui-mod` world
// (wit/horizon.wit, from Horizon, MIT) and the calls between the game and `lite::Lite`.

pub mod lite;

// Only WebAssembly carries the component's imports and exports: a host build (the unit
// tests) checks the logic without them.
#[cfg(target_arch = "wasm32")]
mod host {
    wit_bindgen::generate!({
        path: "wit",
        world: "horizon-ui-mod",
    });

    use exports::horizon::mods::events::{Guest, Registration};
    use exports::horizon::mods::ui_events::Guest as UiGuest;
    use horizon::mods::{chat, command, game, hud, log, render, storage};
    use crate::lite::{Lite, Out, PlayerIn, Shape};
    use std::cell::RefCell;
    use stormfront::math::Vec3;

    thread_local! {
        static LITE: RefCell<Option<Lite>> = const { RefCell::new(None) };
    }

    fn player() -> Option<PlayerIn> {
        game::get_player().map(|p| PlayerIn { pos: Vec3::new(p.x, p.y, p.z), dimension: p.dimension })
    }

    fn apply(out: Out) {
        if !out.commands.is_empty() {
            command::run_quiet(&out.commands);
        }
        for line in out.chat.iter().take(8) {
            chat::show(line);
        }
        if let Some(lines) = out.status {
            if lines.is_empty() {
                hud::clear();
            } else {
                let lines: Vec<hud::Line> = lines.into_iter().map(|(text, color)| hud::Line { text, color }).collect();
                hud::set_panel(hud::Corner::TopLeft, &lines);
            }
        }
        if let Some(radar) = out.radar {
            match radar {
                Some((w, h, shapes)) => {
                    let shapes: Vec<render::Shape> = shapes
                        .into_iter()
                        .map(|s| match s {
                            Shape::Fill { x, y, w, h, argb } => render::Shape::Fill(render::FillShape { rect: render::Rect { x, y, w, h }, color: argb }),
                            Shape::Text { x, y, text, rgb, shadow } => render::Shape::Text(render::TextShape { text, at: render::Point { x, y }, color: rgb, centered: false, shadow }),
                        })
                        .collect();
                    render::set_corner(render::Corner::TopRight, w, h, &shapes);
                }
                None => render::clear_corner(),
            }
        }
        if let Some((strength, seconds)) = out.shake {
            render::shake(strength, seconds);
        }
        if let Some(s) = out.save {
            let _ = storage::set("settings", s.as_bytes());
        }
    }

    struct Component;

    impl Guest for Component {
        fn init() -> Result<Registration, String> {
            let seed = game::get_clock().map_or(0x5703, |c| c.game_time ^ 0x5703_5703);
            let mut lite = Lite::new(seed);
            if let Some(saved) = storage::get("settings") {
                lite.load_settings(&String::from_utf8_lossy(&saved));
            }
            LITE.with(|l| *l.borrow_mut() = Some(lite));
            log::write(log::Level::Info, "Stormfront Lite ready: /storm, /tornado, /radar, /sfx");
            Ok(Registration { commands: vec!["tornado".into(), "storm".into(), "radar".into(), "sfx".into()], tick: true, frame: false, chat: false })
        }

        fn tick(_tick: u64) {
            let day = game::get_clock().map_or(6000, |c| c.day_time);
            let p = player();
            let out = LITE.with(|l| l.borrow_mut().as_mut().map(|lite| lite.tick(p, day)));
            if let Some(out) = out {
                apply(out);
            }
        }

        fn frame(_seconds: f32) {}

        fn command(name: String, args: String) -> Result<Option<String>, String> {
            let mut out = Out::default();
            let reply = LITE.with(|l| match l.borrow_mut().as_mut() {
                Some(lite) => lite.command(&name, &args, &mut out),
                None => Err("Stormfront Lite is not running".into()),
            });
            let lines = reply?;
            apply(out);
            let mut it = lines.into_iter();
            let first = it.next();
            for line in it.take(7) {
                chat::show(&line);
            }
            Ok(first)
        }

        fn chat(_text: String) {}
    }

    impl UiGuest for Component {
        fn select(_id: u32, _row: u32) {}
        fn closed() {}
        fn settings_opened() {}
    }

    export!(Component);
}
