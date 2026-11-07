// A player's /storm or /tornado, on the server.
fn stormfront_perform_command(
    game: &mut crate::game::Game<'_>,
    player: &crate::game::native_server::NativeServerPlayer,
    command: &str,
    ran: &mut bool,
) {
    crate::game::stormfront_glue::perform_command(game, player, command, ran);
}
