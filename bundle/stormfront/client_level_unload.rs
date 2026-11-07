// The window leaves a level: forget its weather.
fn stormfront_client_level_unload(dimension: &str) {
    let _ = dimension;
    crate::game::stormfront_glue::level_unload();
}
