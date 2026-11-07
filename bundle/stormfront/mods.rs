// The mod's row in Options > Mods.
fn stormfront_mods(mods: &mut Vec<crate::game::native_hooks::NativeMod>) {
    crate::game::stormfront_glue::listed(mods);
}
