// Keep the world's depth for soft particles while there are particles.
fn stormfront_render_needs(needs: &mut crate::gfx::native_render::RenderNeeds) {
    crate::stormfront_render::needs(needs);
}
