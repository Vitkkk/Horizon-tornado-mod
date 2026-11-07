// The world is drawn: Stormfront's particles go over it, before the hand and the HUD.
fn stormfront_world_drawn(world: &mut crate::gfx::native_render::WorldDrawn<'_>) {
    crate::stormfront_render::draw(world);
}
