// /storm and /tornado in every player's command tree.
fn stormfront_register_commands(commands: &mut Vec<crate::game::native_server::NativeCommand>) {
    crate::game::stormfront_glue::register_commands(commands);
}
