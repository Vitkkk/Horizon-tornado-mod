// After the Overworld's tick: weather, damage and the messages for the clients.
fn stormfront_level_tick_post(game: &mut crate::game::Game<'_>, dimension: &str) {
    crate::game::stormfront_glue::level_tick(game, dimension);
}
