// The client commands the chat box completes: /radar and /sfx.
fn stormfront_commands(commands: &mut Vec<&'static str>) {
    crate::game::stormfront_glue::commands(commands);
}
