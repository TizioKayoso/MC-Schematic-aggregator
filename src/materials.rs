use anyhow::{Context, Result, bail, ensure};

use crate::litematic::BlockState;

include!(concat!(env!("OUT_DIR"), "/vanilla_registry.rs"));

pub fn required_materials(state: &BlockState) -> Result<Vec<(String, u64)>> {
    let Some(name) = state.name.strip_prefix("minecraft:") else {
        bail!(
            "unsupported block {}: only vanilla Minecraft blocks are supported",
            state.name
        );
    };
    let name = match name {
        "grass" => "short_grass",
        "grass_path" => "dirt_path",
        "chain" => "iron_chain",
        other => other,
    };
    ensure!(
        vanilla_blocks().contains(name),
        "unknown vanilla block {}: supported registry is Minecraft Java 26.3",
        state.name
    );
    let mut materials = block_materials(state, name)?;
    if boolean_property(state, "waterlogged")? == Some(true)
        || matches!(
            name,
            "kelp" | "kelp_plant" | "seagrass" | "tall_seagrass" | "bubble_column"
        )
    {
        materials.push(("minecraft:water_bucket".into(), 1));
    }
    Ok(materials)
}

pub(crate) fn is_vanilla_item(name: &str) -> bool {
    name.strip_prefix("minecraft:")
        .is_some_and(|name| vanilla_items().contains(name))
}

fn vanilla_blocks() -> &'static phf::Set<&'static str> {
    &VANILLA_BLOCKS
}

fn vanilla_items() -> &'static phf::Set<&'static str> {
    &VANILLA_ITEMS
}

fn block_materials(state: &BlockState, name: &str) -> Result<Vec<(String, u64)>> {
    if matches!(
        name,
        "air"
            | "cave_air"
            | "void_air"
            | "fire"
            | "soul_fire"
            | "piston_head"
            | "moving_piston"
            | "nether_portal"
            | "end_portal"
            | "end_gateway"
            | "bubble_column"
            | "frosted_ice"
    ) {
        return Ok(Vec::new());
    }

    if (name.ends_with("_door") || is_double_plant(name))
        && property(state, "half") == Some("upper")
    {
        return Ok(Vec::new());
    }
    if name.ends_with("_bed") && property(state, "part") == Some("head") {
        return Ok(Vec::new());
    }

    if matches!(name, "water" | "lava") {
        let level = numeric_property(state, "level", 0, 15, 0)?;
        return if level == 0 {
            Ok(vec![(format!("minecraft:{name}_bucket"), 1)])
        } else {
            Ok(Vec::new())
        };
    }

    if matches!(
        name,
        "water_cauldron" | "lava_cauldron" | "powder_snow_cauldron"
    ) {
        let contents = match name {
            "water_cauldron" => {
                let level = numeric_property(state, "level", 1, 3, 1)?;
                if level == 3 {
                    ("minecraft:water_bucket".into(), 1)
                } else {
                    ("minecraft:potion[potion=minecraft:water]".into(), level)
                }
            }
            "lava_cauldron" => ("minecraft:lava_bucket".into(), 1),
            "powder_snow_cauldron" => {
                numeric_property(state, "level", 1, 3, 1)?;
                ("minecraft:powder_snow_bucket".into(), 1)
            }
            _ => unreachable!(),
        };
        return Ok(vec![("minecraft:cauldron".into(), 1), contents]);
    }

    if let Some(plant) = name.strip_prefix("potted_") {
        let plant = match plant {
            "azalea_bush" => "azalea",
            "flowering_azalea_bush" => "flowering_azalea",
            other => other,
        };
        return Ok(vec![
            ("minecraft:flower_pot".into(), 1),
            (format!("minecraft:{plant}"), 1),
        ]);
    }

    if name == "candle_cake" || name.ends_with("_candle_cake") {
        let candle = name.strip_suffix("_cake").expect("matched candle cake");
        return Ok(vec![
            ("minecraft:cake".into(), 1),
            (format!("minecraft:{candle}"), 1),
        ]);
    }

    let count = if name.ends_with("_slab") && property(state, "type") == Some("double") {
        2
    } else if name == "snow" {
        numeric_property(state, "layers", 1, 8, 1)?
    } else if name == "turtle_egg" {
        numeric_property(state, "eggs", 1, 4, 1)?
    } else if name == "sea_pickle" {
        numeric_property(state, "pickles", 1, 4, 1)?
    } else if name == "candle" || name.ends_with("_candle") {
        numeric_property(state, "candles", 1, 4, 1)?
    } else if matches!(name, "pink_petals" | "wildflowers") {
        numeric_property(state, "flower_amount", 1, 4, 1)?
    } else if name == "leaf_litter" {
        numeric_property(state, "segment_amount", 1, 4, 1)?
    } else if matches!(name, "glow_lichen" | "sculk_vein" | "resin_clump") {
        face_count(state)?
    } else if name == "vine" {
        vine_count(state)?
    } else {
        1
    };

    if count == 0 {
        return Ok(Vec::new());
    }

    let item = item_name(name);
    let item = format!("minecraft:{item}");
    ensure!(
        is_vanilla_item(&item),
        "no vanilla item mapping for {}",
        state.name
    );
    let mut materials = vec![(item, count)];
    if name == "respawn_anchor" {
        let charges = numeric_property(state, "charges", 0, 4, 0)?;
        if charges != 0 {
            materials.push(("minecraft:glowstone".into(), charges));
        }
    } else if name == "end_portal_frame" && boolean_property(state, "eye")? == Some(true) {
        materials.push(("minecraft:ender_eye".into(), 1));
    }
    Ok(materials)
}

fn property<'a>(state: &'a BlockState, key: &str) -> Option<&'a str> {
    state.properties.get(key).map(String::as_str)
}

fn boolean_property(state: &BlockState, key: &str) -> Result<Option<bool>> {
    match property(state, key) {
        Some("true") => Ok(Some(true)),
        Some("false") => Ok(Some(false)),
        None => Ok(None),
        Some(value) => bail!("invalid {key} property {value:?} for {}", state.name),
    }
}

fn numeric_property(
    state: &BlockState,
    key: &str,
    minimum: u64,
    maximum: u64,
    default: u64,
) -> Result<u64> {
    let Some(value) = property(state, key) else {
        return Ok(default);
    };
    let number = value.parse::<u64>().with_context(|| {
        format!(
            "invalid {key} property {value:?} for {}: expected {minimum}..={maximum}",
            state.name
        )
    })?;
    if !(minimum..=maximum).contains(&number) {
        bail!(
            "invalid {key} property {value:?} for {}: expected {minimum}..={maximum}",
            state.name
        );
    }
    Ok(number)
}

fn face_count(state: &BlockState) -> Result<u64> {
    let mut has_faces = false;
    let mut count = 0;
    for face in ["down", "up", "north", "south", "west", "east"] {
        if let Some(value) = property(state, face) {
            has_faces = true;
            match value {
                "true" => count += 1,
                "false" => {}
                _ => bail!("invalid {face} property {value:?} for {}", state.name),
            }
        }
    }
    Ok(if has_faces { count } else { 1 })
}

fn vine_count(state: &BlockState) -> Result<u64> {
    let mut count = 0;
    for face in ["north", "south", "west", "east"] {
        if boolean_property(state, face)? == Some(true) {
            count += 1;
        }
    }
    // The upper attachment can form automatically from a supporting block.
    boolean_property(state, "up")?;
    Ok(count.max(1))
}

fn is_double_plant(name: &str) -> bool {
    matches!(
        name,
        "sunflower"
            | "lilac"
            | "rose_bush"
            | "peony"
            | "tall_grass"
            | "large_fern"
            | "tall_seagrass"
            | "small_dripleaf"
            | "pitcher_plant"
            | "pitcher_crop"
    )
}

fn item_name(name: &str) -> String {
    let alias = match name {
        "wall_torch" => "torch",
        "redstone_wall_torch" => "redstone_torch",
        "soul_wall_torch" => "soul_torch",
        "copper_wall_torch" => "copper_torch",
        "redstone_wire" => "redstone",
        "tripwire" => "string",
        "farmland" | "dirt_path" | "grass_path" => "dirt",
        "wheat" => "wheat_seeds",
        "carrots" => "carrot",
        "potatoes" => "potato",
        "beetroots" => "beetroot_seeds",
        "cocoa" => "cocoa_beans",
        "sweet_berry_bush" => "sweet_berries",
        "melon_stem" | "attached_melon_stem" => "melon_seeds",
        "pumpkin_stem" | "attached_pumpkin_stem" => "pumpkin_seeds",
        "torchflower_crop" => "torchflower_seeds",
        "pitcher_crop" => "pitcher_pod",
        "bamboo_sapling" => "bamboo",
        "kelp_plant" => "kelp",
        "big_dripleaf_stem" => "big_dripleaf",
        "twisting_vines_plant" => "twisting_vines",
        "weeping_vines_plant" => "weeping_vines",
        "tall_seagrass" => "seagrass",
        "cave_vines" | "cave_vines_plant" => "glow_berries",
        "powder_snow" => "powder_snow_bucket",
        other => other,
    };
    for (suffix, item_suffix) in [
        ("_wall_hanging_sign", "_hanging_sign"),
        ("_wall_sign", "_sign"),
        ("_wall_banner", "_banner"),
        ("_wall_head", "_head"),
        ("_wall_skull", "_skull"),
        ("_coral_wall_fan", "_coral_fan"),
    ] {
        if let Some(base) = alias.strip_suffix(suffix) {
            return format!("{base}{item_suffix}");
        }
    }
    alias.to_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    struct RegisteredBlockState {
        default: BTreeMap<String, String>,
        properties: BTreeMap<String, Vec<String>>,
    }

    fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
        BlockState {
            name: name.into(),
            properties: properties
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect(),
        }
    }

    fn required(name: &str, properties: &[(&str, &str)]) -> Vec<(String, u64)> {
        required_materials(&state(name, properties)).unwrap()
    }

    fn item(name: &str, count: u64) -> Vec<(String, u64)> {
        vec![(name.into(), count)]
    }

    #[test]
    fn counts_multiple_items_in_one_block() {
        for (name, property, value, expected) in [
            ("minecraft:stone_slab", "type", "double", 2),
            ("minecraft:snow", "layers", "8", 8),
            ("minecraft:turtle_egg", "eggs", "4", 4),
            ("minecraft:sea_pickle", "pickles", "3", 3),
            ("minecraft:red_candle", "candles", "2", 2),
        ] {
            assert_eq!(required(name, &[(property, value)]), item(name, expected));
        }
    }

    #[test]
    fn counts_doors_beds_and_tall_plants_only_once() {
        for name in [
            "minecraft:oak_door",
            "minecraft:sunflower",
            "minecraft:small_dripleaf",
            "minecraft:pitcher_crop",
        ] {
            assert!(required(name, &[("half", "upper")]).is_empty());
            assert!(!required(name, &[("half", "lower")]).is_empty());
        }
        assert!(required("minecraft:red_bed", &[("part", "head")]).is_empty());
        assert_eq!(
            required("minecraft:red_bed", &[("part", "foot")]),
            item("minecraft:red_bed", 1)
        );
        assert_eq!(
            required("minecraft:oak_stairs", &[("half", "top")]),
            item("minecraft:oak_stairs", 1)
        );
        assert_eq!(
            required("minecraft:oak_trapdoor", &[]),
            item("minecraft:oak_trapdoor", 1)
        );
    }

    #[test]
    fn separates_pots_and_candle_cakes_into_components() {
        assert_eq!(
            required("minecraft:potted_flowering_azalea_bush", &[]),
            vec![
                ("minecraft:flower_pot".into(), 1),
                ("minecraft:flowering_azalea".into(), 1)
            ]
        );
        assert_eq!(
            required("minecraft:blue_candle_cake", &[]),
            vec![
                ("minecraft:cake".into(), 1),
                ("minecraft:blue_candle".into(), 1)
            ]
        );
    }

    #[test]
    fn translates_placement_aliases() {
        for (block, expected) in [
            ("oak_wall_sign", "oak_sign"),
            ("oak_wall_hanging_sign", "oak_hanging_sign"),
            ("white_wall_banner", "white_banner"),
            ("skeleton_wall_skull", "skeleton_skull"),
            ("player_wall_head", "player_head"),
            ("dead_brain_coral_wall_fan", "dead_brain_coral_fan"),
            ("redstone_wire", "redstone"),
            ("tripwire", "string"),
            ("wheat", "wheat_seeds"),
            ("cocoa", "cocoa_beans"),
            ("sweet_berry_bush", "sweet_berries"),
            ("farmland", "dirt"),
            ("powder_snow", "powder_snow_bucket"),
        ] {
            assert_eq!(
                required(&format!("minecraft:{block}"), &[]),
                item(&format!("minecraft:{expected}"), 1)
            );
        }
    }

    #[test]
    fn fluid_sources_need_buckets_and_flowing_blocks_need_no_extra_items() {
        for fluid in ["water", "lava"] {
            let name = format!("minecraft:{fluid}");
            assert_eq!(
                required(&name, &[("level", "0")]),
                item(&format!("{name}_bucket"), 1)
            );
            assert!(required(&name, &[("level", "1")]).is_empty());
            assert!(required(&name, &[("level", "15")]).is_empty());
        }
    }

    #[test]
    fn skips_air_and_generated_states() {
        for name in [
            "air",
            "cave_air",
            "void_air",
            "fire",
            "soul_fire",
            "piston_head",
            "moving_piston",
            "nether_portal",
            "end_portal",
            "end_gateway",
            "frosted_ice",
        ] {
            assert!(required(&format!("minecraft:{name}"), &[]).is_empty());
        }
    }

    #[test]
    fn validates_numeric_properties_instead_of_silently_undercounting() {
        for (name, key, value) in [
            ("minecraft:snow", "layers", "0"),
            ("minecraft:snow", "layers", "9"),
            ("minecraft:turtle_egg", "eggs", "-1"),
            ("minecraft:sea_pickle", "pickles", "five"),
            ("minecraft:candle", "candles", "5"),
            ("minecraft:lava", "level", "16"),
        ] {
            let message = required_materials(&state(name, &[(key, value)]))
                .unwrap_err()
                .to_string();
            assert!(message.contains(key));
            assert!(message.contains(name));
        }
    }

    #[test]
    fn counts_multiface_items_and_rejects_unknown_or_modded_ids() {
        assert_eq!(
            required(
                "minecraft:glow_lichen",
                &[("up", "true"), ("north", "true"), ("east", "false")]
            ),
            item("minecraft:glow_lichen", 2)
        );
        for name in [
            "minecraft:new_block",
            "custom:air",
            "custom:stone_slab",
            "custom:oak_door",
            "custom:potted_fern",
        ] {
            assert!(
                required_materials(&state(
                    name,
                    &[
                        ("type", "double"),
                        ("half", "upper"),
                        ("candles", "nonnumeric")
                    ]
                ))
                .is_err()
            );
        }
    }

    #[test]
    fn covers_every_vanilla_block_and_property_value() {
        assert_eq!(vanilla_blocks().len(), 1286);
        assert_eq!(vanilla_items().len(), 1658);
        let states: BTreeMap<String, RegisteredBlockState> =
            serde_json::from_str(include_str!("../data/vanilla_block_states_26_3.json")).unwrap();
        let mut covered = BTreeSet::new();
        for (name, registered) in &states {
            assert!(covered.insert(name), "duplicate block defaults for {name}");
            let properties: Vec<_> = registered
                .default
                .iter()
                .map(|(property, value)| (property.as_str(), value.as_str()))
                .collect();
            let default_state = state(&format!("minecraft:{name}"), &properties);
            assert_registered_materials(&default_state);
            for (property, values) in &registered.properties {
                assert!(values.contains(&registered.default[property]));
                for value in values {
                    let mut variant = default_state.clone();
                    variant.properties.insert(property.into(), value.into());
                    assert_registered_materials(&variant);
                }
            }
        }
        assert_eq!(
            covered
                .into_iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            vanilla_blocks().iter().copied().collect()
        );
    }

    #[test]
    fn compiled_registries_match_json_data() {
        for (data, registry) in [
            (
                include_str!("../data/vanilla_blocks_26_3.json"),
                vanilla_blocks(),
            ),
            (
                include_str!("../data/vanilla_items_26_3.json"),
                vanilla_items(),
            ),
        ] {
            let names: Vec<&str> = serde_json::from_str(data).unwrap();
            let unique_names: BTreeSet<_> = names.iter().copied().collect();
            assert_eq!(names.len(), unique_names.len());
            assert_eq!(unique_names, registry.iter().copied().collect());
        }
    }

    fn assert_registered_materials(state: &BlockState) {
        let materials = required_materials(state).unwrap();
        for (item, count) in materials {
            let item = item.split('[').next().unwrap();
            assert!(
                is_vanilla_item(item),
                "{} maps to unknown item {item}",
                state.name
            );
            assert_ne!(count, 0, "{} contains a zero item count", state.name);
        }
    }

    #[test]
    fn counts_new_and_legacy_vanilla_block_variants() {
        for (block, expected) in [
            ("copper_wall_torch", "copper_torch"),
            ("big_dripleaf_stem", "big_dripleaf"),
            ("twisting_vines_plant", "twisting_vines"),
            ("weeping_vines_plant", "weeping_vines"),
            ("grass", "short_grass"),
            ("grass_path", "dirt"),
            ("chain", "iron_chain"),
            ("poplar_wall_sign", "poplar_sign"),
            ("shelf_mushroom", "shelf_mushroom"),
        ] {
            assert_eq!(
                required(&format!("minecraft:{block}"), &[]),
                item(&format!("minecraft:{expected}"), 1)
            );
        }
        for (name, key, count) in [
            ("pink_petals", "flower_amount", "4"),
            ("wildflowers", "flower_amount", "3"),
            ("leaf_litter", "segment_amount", "2"),
        ] {
            assert_eq!(
                required(&format!("minecraft:{name}"), &[(key, count)]),
                item(&format!("minecraft:{name}"), count.parse().unwrap())
            );
        }
        assert_eq!(
            required("minecraft:black_wool_slab", &[("type", "double")]),
            item("minecraft:black_wool_slab", 2)
        );
        assert_eq!(
            required("minecraft:red_concrete_slab", &[("type", "double")]),
            item("minecraft:red_concrete_slab", 2)
        );
        assert!(required("minecraft:straw_bed", &[("part", "head")]).is_empty());
        assert_eq!(
            required("minecraft:straw_bed", &[("part", "foot")]),
            item("minecraft:straw_bed", 1)
        );
    }

    #[test]
    fn counts_waterlogged_and_intrinsically_aquatic_sources() {
        assert_eq!(
            required(
                "minecraft:oak_slab",
                &[("type", "double"), ("waterlogged", "true")]
            ),
            vec![
                ("minecraft:oak_slab".into(), 2),
                ("minecraft:water_bucket".into(), 1)
            ]
        );
        assert_eq!(
            required("minecraft:kelp_plant", &[]),
            vec![
                ("minecraft:kelp".into(), 1),
                ("minecraft:water_bucket".into(), 1)
            ]
        );
        assert_eq!(
            required("minecraft:bubble_column", &[]),
            item("minecraft:water_bucket", 1)
        );
        assert_eq!(
            required("minecraft:tall_seagrass", &[("half", "upper")]),
            item("minecraft:water_bucket", 1)
        );
        assert!(required_materials(&state("minecraft:chest", &[("waterlogged", "yes")])).is_err());
    }

    #[test]
    fn includes_cauldron_and_respawn_anchor_contents() {
        assert_eq!(
            required("minecraft:water_cauldron", &[("level", "3")]),
            vec![
                ("minecraft:cauldron".into(), 1),
                ("minecraft:water_bucket".into(), 1)
            ]
        );
        assert_eq!(
            required("minecraft:water_cauldron", &[("level", "2")]),
            vec![
                ("minecraft:cauldron".into(), 1),
                ("minecraft:potion[potion=minecraft:water]".into(), 2)
            ]
        );
        assert_eq!(
            required("minecraft:lava_cauldron", &[]),
            vec![
                ("minecraft:cauldron".into(), 1),
                ("minecraft:lava_bucket".into(), 1)
            ]
        );
        assert_eq!(
            required("minecraft:powder_snow_cauldron", &[("level", "1")]),
            vec![
                ("minecraft:cauldron".into(), 1),
                ("minecraft:powder_snow_bucket".into(), 1)
            ]
        );
        assert_eq!(
            required("minecraft:respawn_anchor", &[("charges", "4")]),
            vec![
                ("minecraft:respawn_anchor".into(), 1),
                ("minecraft:glowstone".into(), 4)
            ]
        );
        assert_eq!(
            required("minecraft:end_portal_frame", &[("eye", "true")]),
            vec![
                ("minecraft:end_portal_frame".into(), 1),
                ("minecraft:ender_eye".into(), 1)
            ]
        );
    }

    #[test]
    fn vine_upper_attachment_does_not_add_an_item() {
        assert_eq!(
            required(
                "minecraft:vine",
                &[("up", "true"), ("east", "true"), ("north", "true")]
            ),
            item("minecraft:vine", 2)
        );
        assert_eq!(
            required("minecraft:vine", &[("up", "true")]),
            item("minecraft:vine", 1)
        );
    }
}
