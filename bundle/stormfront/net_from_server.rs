// The server half's weather, debris and effects reach the window.
fn stormfront_net_from_server(game: &mut crate::game::Game<'_>, channel: &str, data: &[u8]) {
    crate::game::stormfront_glue::from_server(game, channel, data);
}
