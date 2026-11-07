// Every game's tick; the glue acts on the window's only.
fn stormfront_tick(game: &mut crate::game::Game<'_>) {
    crate::game::stormfront_glue::tick(game);
}
