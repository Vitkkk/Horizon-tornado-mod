// A world opens or closes: its weather starts over.
fn stormfront_world_lifecycle(phase: horizon_client::mod_api::WorldPhase) {
    crate::game::stormfront_glue::lifecycle(phase);
}
