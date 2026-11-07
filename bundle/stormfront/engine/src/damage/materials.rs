// Material properties of blocks, matched from their ids (vanilla's and mods' alike, by name
// patterns). Strengths are wind pressures (Pa) at which an exposed block with no help from its
// neighbours lets go; neighbours, anchoring and construction quality raise that. They are
// calibrated against the expected winds of the EF scale's degrees of damage (glass ~35 m/s,
// roof covering ~35-43 m/s, roof structure ~55 m/s, frame walls ~60-68 m/s, well-built
// masonry and concrete far higher).

use crate::units::dynamic_pressure;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Air,
    Fluid,
    /// Short plants, flowers, grass, crops, saplings.
    Plant,
    Leaves,
    /// Tree trunks and branches.
    Log,
    /// Debarked (stripped) logs.
    StrippedLog,
    Glass,
    GlassPane,
    /// Light wooden construction: planks, slabs, stairs, doors, fences.
    Wood,
    /// Cloth, wool, carpets, hay: light and weak.
    Light,
    Masonry,
    Stone,
    Concrete,
    Metal,
    /// Soils that scour: grass block, dirt, podzol, mud, farmland, path.
    Soil,
    Sand,
    Gravel,
    Snow,
    Ice,
    /// Small attached things: torches, signs, buttons, rails, lanterns.
    Attachment,
    /// Furniture-like blocks: beds, chests, tables, furnaces, bookshelves.
    Furniture,
    Unbreakable,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    pub kind: Kind,
    /// Mass (kg) of a whole block of it as a piece of construction (a 1 m^2 wall section, a
    /// 1 m length of trunk), not of a solid cube; `block_mass` is this times `fill`.
    pub density: f64,
    /// Fraction of the block the shape fills (slab 0.5, pane 0.12...).
    pub fill: f64,
    /// Wind pressure (Pa) at which an exposed, unsupported block fails.
    pub strength: f64,
    /// How much each supporting neighbour adds to the strength (fraction).
    pub connect: f64,
    /// Drag coefficient as debris.
    pub cd: f64,
    /// Impact energy (J) that breaks the block when debris hits it.
    pub impact_energy: f64,
    /// Specific energy (J/kg) at which a piece of it breaks apart on impact.
    pub fragment_energy: f64,
    /// Whether it can become flying debris (else it shatters into particles).
    pub debris: bool,
    /// Whether it counts as natural ground for finding the terrain under structures.
    pub terrain: bool,
}

impl Material {
    const fn new(kind: Kind, density: f64, fill: f64, strength: f64, connect: f64, cd: f64, impact: f64, frag: f64, debris: bool, terrain: bool) -> Self {
        Material { kind, density, fill, strength, connect, cd, impact_energy: impact, fragment_energy: frag, debris, terrain }
    }

    pub fn is_air(&self) -> bool {
        self.kind == Kind::Air
    }
    /// Solid enough to stop debris and to carry loads.
    pub fn is_solid(&self) -> bool {
        !matches!(self.kind, Kind::Air | Kind::Fluid | Kind::Plant | Kind::Attachment) && self.fill >= 0.3
    }
    /// Takes part in structural support (not terrain, vegetation or loose stuff).
    /// Debris strikes it (thin panes, fences and doors included), rather than passing through.
    pub fn is_obstacle(&self) -> bool {
        !matches!(self.kind, Kind::Air | Kind::Fluid | Kind::Plant | Kind::Attachment)
    }
    pub fn is_structural(&self) -> bool {
        matches!(self.kind, Kind::Wood | Kind::Masonry | Kind::Stone | Kind::Concrete | Kind::Metal | Kind::Glass | Kind::GlassPane | Kind::Light | Kind::Furniture)
            && !self.terrain
    }
    pub fn is_vegetation(&self) -> bool {
        matches!(self.kind, Kind::Plant | Kind::Leaves | Kind::Log | Kind::StrippedLog)
    }
    /// Mass (kg) of one block of it.
    pub fn block_mass(&self) -> f64 {
        self.density * self.fill
    }
    /// The wind speed (m/s) whose dynamic pressure equals the strength, for reports.
    pub fn failure_speed(&self, rho: f64) -> f64 {
        (2.0 * self.strength / rho).sqrt()
    }
}

pub const AIR: Material = Material::new(Kind::Air, 0.0, 0.0, f64::INFINITY, 0.0, 0.0, 0.0, 0.0, false, false);
pub const FLUID: Material = Material::new(Kind::Fluid, 1000.0, 1.0, f64::INFINITY, 0.0, 0.0, f64::INFINITY, 0.0, false, false);
pub const UNBREAKABLE: Material = Material::new(Kind::Unbreakable, 3000.0, 1.0, f64::INFINITY, 0.0, 1.0, f64::INFINITY, f64::INFINITY, false, true);

/// The material of a block from its id or full state (`minecraft:oak_stairs[facing=east]`).
pub fn classify(state: &str) -> Material {
    let id = state.split('[').next().unwrap_or(state);
    let name = id.rsplit(':').next().unwrap_or(id);
    let has = |s: &str| name.contains(s);
    let ends = |s: &str| name.ends_with(s);
    use Kind::*;
    // order matters: specific patterns first
    if matches!(name, "air" | "cave_air" | "void_air" | "light" | "structure_void") {
        return AIR;
    }
    if matches!(name, "water" | "lava" | "bubble_column") {
        return FLUID;
    }
    if matches!(name, "bedrock" | "barrier" | "end_portal_frame" | "command_block" | "reinforced_deepslate" | "end_portal" | "nether_portal") {
        return UNBREAKABLE;
    }
    if has("glass_pane") {
        return Material::new(GlassPane, 25.0, 0.12, 650.0, 0.15, 1.1, 40.0, 5.0, false, false);
    }
    if has("glass") {
        return Material::new(Glass, 10.0, 1.0, 750.0, 0.25, 1.0, 120.0, 5.0, false, false);
    }
    if ends("_leaves") || name == "azalea_leaves" || name == "flowering_azalea_leaves" {
        return Material::new(Leaves, 5.0, 1.0, 420.0, 0.35, 1.0, 25.0, 50.0, false, false);
    }
    if name.starts_with("stripped_") && (ends("_log") || ends("_wood") || ends("_stem") || ends("_hyphae")) {
        return Material::new(StrippedLog, 140.0, 1.0, 9000.0, 0.6, 1.0, 9000.0, 2000.0, true, false);
    }
    if ends("_log") || ends("_wood") || ends("_stem") && !has("mushroom") && name != "melon_stem" && name != "pumpkin_stem" || ends("_hyphae") || name == "mangrove_roots" {
        return Material::new(Log, 150.0, 1.0, 9000.0, 0.6, 1.0, 9000.0, 2000.0, true, false);
    }
    if matches!(name, "short_grass" | "tall_grass" | "fern" | "large_fern" | "dead_bush" | "seagrass" | "tall_seagrass" | "sweet_berry_bush" | "bush" | "firefly_bush" | "short_dry_grass" | "tall_dry_grass" | "leaf_litter" | "wildflowers" | "pink_petals" | "vine" | "glow_lichen" | "moss_carpet" | "pale_moss_carpet" | "hanging_roots" | "sugar_cane" | "cactus" | "bamboo" | "kelp" | "kelp_plant" | "lily_pad" | "cocoa" | "spore_blossom" | "big_dripleaf" | "small_dripleaf" | "azalea" | "flowering_azalea" | "torchflower" | "pitcher_plant" | "cactus_flower" | "eyeblossom" | "open_eyeblossom" | "closed_eyeblossom")
        || ends("_sapling")
        || ends("_tulip")
        || ends("_flower") && !has("pot")
        || ends("_mushroom") && !ends("_block")
        || matches!(name, "poppy" | "dandelion" | "blue_orchid" | "allium" | "azure_bluet" | "oxeye_daisy" | "cornflower" | "lily_of_the_valley" | "wither_rose" | "sunflower" | "lilac" | "rose_bush" | "peony" | "pitcher_crop" | "torchflower_crop")
        || matches!(name, "wheat" | "carrots" | "potatoes" | "beetroots" | "melon_stem" | "pumpkin_stem" | "attached_melon_stem" | "attached_pumpkin_stem" | "nether_wart")
    {
        return Material::new(Plant, 5.0, 0.2, 900.0, 0.0, 1.2, 5.0, 10.0, false, false);
    }
    if matches!(name, "torch" | "wall_torch" | "soul_torch" | "soul_wall_torch" | "redstone_torch" | "redstone_wall_torch" | "lever" | "tripwire" | "tripwire_hook" | "redstone_wire" | "snow" | "flower_pot" | "candle" | "chain" | "iron_chain" | "lantern" | "soul_lantern" | "ladder" | "scaffolding")
        || ends("_sign")
        || ends("_button")
        || ends("_pressure_plate")
        || ends("rail")
        || ends("_banner")
        || ends("_carpet")
        || ends("_candle")
        || name.starts_with("potted_")
        || ends("_head")
        || ends("_skull")
    {
        let snow = name == "snow";
        return Material::new(if snow { Snow } else { Attachment }, if snow { 40.0 } else { 20.0 }, 0.1, 700.0, 0.2, 1.2, 15.0, 20.0, false, false);
    }
    if has("wool") || matches!(name, "hay_block" | "white_bed") || ends("_bed") || name == "target" || name == "dried_kelp_block" {
        return Material::new(Light, 25.0, if ends("_bed") { 0.56 } else { 1.0 }, 900.0, 0.3, 1.1, 600.0, 100.0, true, false);
    }
    if ends("_door") || ends("_trapdoor") || ends("_fence_gate") {
        let iron = name.starts_with("iron_") || has("copper");
        return Material::new(Wood, if iron { 320.0 } else { 160.0 }, 0.19, if iron { 2400.0 } else { 1000.0 }, 0.4, 1.2, if iron { 12_000.0 } else { 4000.0 }, 800.0, true, false);
    }
    if ends("_fence") && !has("nether_brick") || ends("_wall") && has("wood") || ends("_pane") {
        return Material::new(Wood, 35.0, 0.3, 1000.0, 0.4, 1.2, 3000.0, 800.0, true, false);
    }
    let wood_names = ["oak", "spruce", "birch", "jungle", "acacia", "dark_oak", "mangrove", "cherry", "bamboo", "crimson", "warped", "pale_oak"];
    let woodish = ends("_planks") || wood_names.iter().any(|w| name.starts_with(w) && (ends("_slab") || ends("_stairs"))) || matches!(name, "bamboo_mosaic" | "bamboo_mosaic_slab" | "bamboo_mosaic_stairs" | "bamboo_block");
    if woodish {
        let (fill, strength) = if ends("_slab") {
            (0.5, 800.0)
        } else if ends("_stairs") {
            (0.75, 900.0)
        } else {
            (1.0, 1300.0)
        };
        return Material::new(Wood, 45.0, fill, strength, 0.4, 1.1, 8000.0 * fill, 600.0, true, false);
    }
    if matches!(name, "chest" | "trapped_chest" | "barrel" | "crafting_table" | "furnace" | "blast_furnace" | "smoker" | "bookshelf" | "chiseled_bookshelf" | "lectern" | "composter" | "loom" | "cartography_table" | "fletching_table" | "smithing_table" | "beehive" | "bee_nest" | "note_block" | "jukebox" | "enchanting_table" | "brewing_stand" | "cauldron" | "grindstone" | "stonecutter" | "anvil" | "chipped_anvil" | "damaged_anvil" | "bell" | "campfire" | "soul_campfire" | "decorated_pot" | "crafter")
        || ends("shulker_box")
    {
        let heavy = has("anvil") || has("furnace") || name == "smoker" || name == "cauldron";
        return Material::new(Furniture, if heavy { 190.0 } else { 60.0 }, 0.8, if heavy { 5000.0 } else { 1500.0 }, 0.3, 1.1, 6000.0, 900.0, true, false);
    }
    if has("concrete") && !has("powder") || has("obsidian") || name == "crying_obsidian" || name == "respawn_anchor" {
        return Material::new(Concrete, 500.0, 1.0, 4500.0, 1.0, 1.0, 150_000.0, 3000.0, true, false);
    }
    if matches!(name, "iron_block" | "gold_block" | "netherite_block" | "copper_block" | "raw_iron_block" | "raw_copper_block" | "raw_gold_block" | "heavy_core" | "lodestone" | "iron_bars")
        || has("copper") && !has("ore")
    {
        let bars = name == "iron_bars" || ends("_bars") || ends("_grate");
        return Material::new(Metal, if bars { 66.0 } else { 400.0 }, if bars { 0.15 } else { 1.0 }, if bars { 2500.0 } else { 6000.0 }, 0.8, 1.0, 150_000.0, 8000.0, true, false);
    }
    if has("brick") || ends("terracotta") || matches!(name, "mud_bricks" | "packed_mud") || has("tile") {
        let (fill, mult) = if ends("_slab") {
            (0.5, 0.75)
        } else if ends("_stairs") {
            (0.75, 0.85)
        } else if ends("_wall") {
            (0.5, 0.8)
        } else {
            (1.0, 1.0)
        };
        return Material::new(Masonry, 200.0, fill, 1700.0 * mult, 0.6, 1.0, 30_000.0 * fill, 1200.0, true, false);
    }
    if matches!(name, "grass_block" | "dirt" | "coarse_dirt" | "rooted_dirt" | "podzol" | "mycelium" | "mud" | "farmland" | "dirt_path" | "moss_block" | "pale_moss_block" | "clay" | "muddy_mangrove_roots") {
        return Material::new(Soil, 1500.0, 1.0, 30_000.0, 1.0, 1.0, 30_000.0, 300.0, false, true);
    }
    if matches!(name, "sand" | "red_sand" | "suspicious_sand") {
        return Material::new(Sand, 1600.0, 1.0, 30_000.0, 1.0, 1.0, 30_000.0, 100.0, false, true);
    }
    if matches!(name, "gravel" | "suspicious_gravel") {
        return Material::new(Gravel, 1800.0, 1.0, 40_000.0, 1.0, 1.0, 40_000.0, 200.0, false, true);
    }
    if matches!(name, "snow_block" | "powder_snow") {
        return Material::new(Snow, 400.0, 1.0, 2000.0, 0.5, 1.0, 1000.0, 50.0, false, true);
    }
    if has("ice") {
        return Material::new(Ice, 300.0, 1.0, 6000.0, 0.8, 1.0, 5000.0, 400.0, true, true);
    }
    // natural rock counts as terrain; worked stone is masonry-like but stronger
    let natural_rock = matches!(name, "stone" | "deepslate" | "granite" | "diorite" | "andesite" | "tuff" | "calcite" | "dripstone_block" | "sandstone" | "red_sandstone" | "netherrack" | "end_stone" | "basalt" | "blackstone" | "smooth_basalt" | "magma_block" | "soul_sand" | "soul_soil" | "amethyst_block")
        || ends("_ore");
    if natural_rock {
        return Material::new(Stone, 2600.0, 1.0, 60_000.0, 1.0, 1.0, 200_000.0, 4000.0, true, true);
    }
    if has("stone") || has("deepslate") || has("cobble") || has("andesite") || has("diorite") || has("granite") || has("quartz") || has("prismarine") || has("purpur") || has("blackstone") || has("sandstone") || has("tuff") {
        let (fill, mult) = if ends("_slab") {
            (0.5, 0.75)
        } else if ends("_stairs") {
            (0.75, 0.85)
        } else if ends("_wall") {
            (0.5, 0.8)
        } else {
            (1.0, 1.0)
        };
        return Material::new(Stone, 250.0, fill, 2200.0 * mult, 0.7, 1.0, 50_000.0 * fill, 2500.0, true, false);
    }
    if has("pumpkin") || has("melon") || ends("_block") && (has("mushroom") || has("wart")) || name == "shroomlight" {
        return Material::new(Light, 30.0, 1.0, 1400.0, 0.3, 1.0, 600.0, 200.0, true, false);
    }
    // anything else: a generic, moderately strong solid
    Material::new(Other, 120.0, 1.0, 3000.0, 0.6, 1.1, 10_000.0, 1000.0, true, false)
}

/// Which tree species a log or leaves block belongs to, for its critical wind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Species {
    Oak,
    Birch,
    Spruce,
    Jungle,
    Acacia,
    DarkOak,
    Mangrove,
    Cherry,
    PaleOak,
    Other,
}

impl Species {
    pub fn of(state: &str) -> Species {
        let n = state.split('[').next().unwrap_or(state);
        let n = n.rsplit(':').next().unwrap_or(n);
        let n = n.trim_start_matches("stripped_");
        if n.starts_with("dark_oak") {
            Species::DarkOak
        } else if n.starts_with("pale_oak") {
            Species::PaleOak
        } else if n.starts_with("oak") {
            Species::Oak
        } else if n.starts_with("birch") {
            Species::Birch
        } else if n.starts_with("spruce") {
            Species::Spruce
        } else if n.starts_with("jungle") {
            Species::Jungle
        } else if n.starts_with("acacia") {
            Species::Acacia
        } else if n.starts_with("mangrove") {
            Species::Mangrove
        } else if n.starts_with("cherry") {
            Species::Cherry
        } else {
            Species::Other
        }
    }
    /// The wood prefix of its block ids (`oak` for oak_log).
    pub fn prefix(self) -> &'static str {
        match self {
            Species::Oak | Species::Other => "oak",
            Species::Birch => "birch",
            Species::Spruce => "spruce",
            Species::Jungle => "jungle",
            Species::Acacia => "acacia",
            Species::DarkOak => "dark_oak",
            Species::Mangrove => "mangrove",
            Species::Cherry => "cherry",
            Species::PaleOak => "pale_oak",
        }
    }
    /// Wind at the crown (m/s) that snaps a typical full-crowned trunk, and whether its roots
    /// are shallow (it uproots rather than snaps in wet soil). Softwoods and birch are weaker.
    pub fn critical_wind(self) -> (f64, bool) {
        match self {
            Species::Oak => (47.0, false),
            Species::Birch => (41.0, false),
            Species::Spruce => (44.0, true),
            Species::Jungle => (50.0, false),
            Species::Acacia => (46.0, true),
            Species::DarkOak => (58.0, false),
            Species::Mangrove => (52.0, true),
            Species::Cherry => (43.0, false),
            Species::PaleOak => (55.0, false),
            Species::Other => (45.0, false),
        }
    }
    pub fn is_softwood(self) -> bool {
        matches!(self, Species::Spruce)
    }
}

/// What a removed block leaves on the ground when its debris comes to rest, if anything:
/// boards (slabs) from planks, broken masonry from bricks, a log lying along `axis`.
pub fn deposit_state(state: &str, axis: char) -> Option<String> {
    let id = state.split('[').next().unwrap_or(state);
    let (ns, name) = match id.split_once(':') {
        Some((a, b)) => (a, b),
        None => ("minecraft", id),
    };
    let m = classify(state);
    match m.kind {
        Kind::Wood => {
            let wood = ["oak", "spruce", "birch", "jungle", "acacia", "dark_oak", "mangrove", "cherry", "pale_oak", "crimson", "warped"]
                .iter()
                .find(|w| name.starts_with(&format!("{w}_")))
                .copied();
            wood.map(|w| format!("{ns}:{w}_slab[type=bottom]"))
        }
        Kind::Log | Kind::StrippedLog => Some(format!("{ns}:{name}[axis={axis}]")),
        Kind::Masonry => Some(if name.contains("mud") {
            "minecraft:mud_brick_slab[type=bottom]".to_string()
        } else if name.contains("brick") {
            "minecraft:brick_slab[type=bottom]".to_string()
        } else {
            "minecraft:cobblestone_slab[type=bottom]".to_string()
        }),
        Kind::Stone | Kind::Concrete => Some("minecraft:cobblestone_slab[type=bottom]".to_string()),
        Kind::Light if name.contains("hay") => Some("minecraft:hay_block".to_string()),
        _ => None,
    }
}

/// Speed (m/s) at which an exposed block of strength `s` (Pa) fails: inverse of q = rho V^2/2.
pub fn speed_for_strength(s: f64, rho: f64) -> f64 {
    (2.0 * s / rho).sqrt()
}

/// Dynamic pressure helper re-exported for the damage passes.
pub fn q(rho: f64, v: f64) -> f64 {
    dynamic_pressure(rho, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_of_common_blocks() {
        assert_eq!(classify("minecraft:air").kind, Kind::Air);
        assert_eq!(classify("minecraft:glass_pane[east=true]").kind, Kind::GlassPane);
        assert_eq!(classify("minecraft:white_stained_glass").kind, Kind::Glass);
        assert_eq!(classify("minecraft:oak_leaves[distance=1]").kind, Kind::Leaves);
        assert_eq!(classify("minecraft:oak_log[axis=y]").kind, Kind::Log);
        assert_eq!(classify("minecraft:stripped_birch_log[axis=y]").kind, Kind::StrippedLog);
        assert_eq!(classify("minecraft:spruce_planks").kind, Kind::Wood);
        assert_eq!(classify("minecraft:oak_stairs[facing=east]").kind, Kind::Wood);
        assert_eq!(classify("minecraft:bricks").kind, Kind::Masonry);
        assert_eq!(classify("minecraft:stone_bricks").kind, Kind::Masonry);
        assert_eq!(classify("minecraft:cobblestone").kind, Kind::Stone);
        assert_eq!(classify("minecraft:white_concrete").kind, Kind::Concrete);
        assert_eq!(classify("minecraft:iron_block").kind, Kind::Metal);
        assert_eq!(classify("minecraft:grass_block[snowy=false]").kind, Kind::Soil);
        assert_eq!(classify("minecraft:poppy").kind, Kind::Plant);
        assert_eq!(classify("minecraft:short_grass").kind, Kind::Plant);
        assert_eq!(classify("minecraft:stone").kind, Kind::Stone);
        assert!(classify("minecraft:stone").terrain);
        assert!(!classify("minecraft:cobblestone").terrain);
        assert_eq!(classify("minecraft:bedrock").kind, Kind::Unbreakable);
        assert_eq!(classify("minecraft:water[level=0]").kind, Kind::Fluid);
        assert_eq!(classify("mymod:strange_block").kind, Kind::Other);
    }

    #[test]
    fn failure_speeds_are_ordered_like_the_ef_degrees_of_damage() {
        let v = |s: &str| classify(s).failure_speed(1.2);
        assert!(v("minecraft:glass_pane") < v("minecraft:poppy"));
        assert!(v("minecraft:poppy") < v("minecraft:oak_planks"));
        assert!(v("minecraft:glass") < v("minecraft:oak_slab"));
        assert!(v("minecraft:oak_slab") < v("minecraft:oak_planks"));
        assert!(v("minecraft:oak_planks") < v("minecraft:bricks"));
        assert!(v("minecraft:bricks") < v("minecraft:white_concrete"));
        // an exposed, unsupported block: glass near 35 m/s, a plank near 47 m/s (support and
        // anchoring raise both in a building)
        assert!((30.0..40.0).contains(&v("minecraft:glass")));
        assert!((40.0..52.0).contains(&v("minecraft:oak_planks")));
    }

    #[test]
    fn species_and_deposits() {
        assert_eq!(Species::of("minecraft:dark_oak_log[axis=y]"), Species::DarkOak);
        assert_eq!(Species::of("minecraft:stripped_spruce_log"), Species::Spruce);
        assert_eq!(deposit_state("minecraft:oak_planks", 'x').as_deref(), Some("minecraft:oak_slab[type=bottom]"));
        assert_eq!(deposit_state("minecraft:birch_log[axis=y]", 'z').as_deref(), Some("minecraft:birch_log[axis=z]"));
        assert_eq!(deposit_state("minecraft:glass", 'x'), None);
        assert_eq!(deposit_state("minecraft:bricks", 'x').as_deref(), Some("minecraft:brick_slab[type=bottom]"));
    }
}
