// The radar panel, tornado warnings and the scientific readout.
fn stormfront_hud(
    game: &mut crate::game::Game<'_>,
    batch: &mut horizon_client::hud::HudBatch,
    corners: &mut horizon_client::extras_hud::Corners,
) {
    crate::game::stormfront_glue::hud(game, batch, corners);
}
