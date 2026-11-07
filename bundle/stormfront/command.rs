// /radar and /sfx, on the client.
fn stormfront_command(game: &mut crate::game::Game<'_>, command: &str, ran: &mut bool) {
    crate::game::stormfront_glue::command(game, command, ran);
}
