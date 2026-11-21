/// The checked-in Cargo tier map is shared by restart detection and build
/// scripts. Cargo cannot invoke cargo metadata from inside a build script.
pub const TIER_MAP_PATH: &str = "crates/crate-tiers.tsv";
pub const ENGINE_CRATE_PATHS_SOURCE: &str = "crates/engine_crate_paths.rs";
pub const ENGINE_INVENTORY_INPUTS: &[&str] = &[TIER_MAP_PATH, ENGINE_CRATE_PATHS_SOURCE];
const TIER_MAP: &str = include_str!("crate-tiers.tsv");

pub fn engine_crates() -> impl Iterator<Item = &'static str> {
    TIER_MAP.lines().filter_map(|line| {
        let mut fields = line.split('\t');
        let name = fields.next()?;
        fields.next()?;
        matches!(fields.next(), Some("engine" | "leaf")).then_some(name)
    })
}
