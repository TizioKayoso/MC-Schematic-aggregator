use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result, bail, ensure};
use fastnbt::Value;

type Compound = HashMap<String, Value>;

const MAX_NESTING: usize = 64;

pub fn add_block_entity(counts: &mut BTreeMap<String, u64>, value: &Value) -> Result<()> {
    add_block_entity_scaled(counts, value, 1, 0)
}

pub fn add_item_stack(counts: &mut BTreeMap<String, u64>, stack: &Value) -> Result<()> {
    add_stack(counts, stack, 1, 0)
}

pub fn add_entity(counts: &mut BTreeMap<String, u64>, value: &Value) -> Result<()> {
    add_entity_with_contents(counts, value, true)
}

pub fn add_entity_with_contents(
    counts: &mut BTreeMap<String, u64>,
    value: &Value,
    include_contents: bool,
) -> Result<()> {
    add_entity_at_depth(counts, value, include_contents, 0)
}

fn add_block_entity_scaled(
    counts: &mut BTreeMap<String, u64>,
    value: &Value,
    multiplier: u64,
    depth: usize,
) -> Result<()> {
    let data = payload(value, depth)?;
    ensure!(
        !data.contains_key("LootTable") && !data.contains_key("loot_table"),
        "container contents are unresolved: generate its loot table before saving the schematic or use --exclude-contents"
    );
    add_inventory(counts, data, multiplier, depth)?;
    for names in [
        ["Book", "book"],
        ["RecordItem", "record_item"],
        ["item", "Item"],
    ] {
        if let Some(stack) = field(data, &names) {
            add_stack(counts, stack, multiplier, depth + 1)
                .with_context(|| format!("invalid {} item stack", names[0]))?;
        }
    }
    Ok(())
}

fn add_inventory(
    counts: &mut BTreeMap<String, u64>,
    data: &Compound,
    multiplier: u64,
    depth: usize,
) -> Result<()> {
    if let Some(items) = field(data, &["Items", "items", "Inventory", "inventory"]) {
        add_stacks(counts, items, multiplier, depth + 1)?;
    } else if let Some(Value::Compound(components)) = data.get("components")
        && let Some(items) = component(components, "container")
    {
        add_container_slots(counts, items, multiplier, depth + 1)?;
    }
    Ok(())
}

fn add_stack(
    counts: &mut BTreeMap<String, u64>,
    value: &Value,
    multiplier: u64,
    depth: usize,
) -> Result<()> {
    ensure!(
        depth <= MAX_NESTING,
        "item contents exceed {MAX_NESTING} nesting levels"
    );
    let data = as_compound(value, "item stack")?;
    if data.is_empty() {
        return Ok(());
    }
    let count = field(data, &["count", "Count"])
        .map(|value| nonnegative_integer(value, "item count"))
        .transpose()?
        .unwrap_or(1);
    if count == 0 {
        return Ok(());
    }
    let id = data.get("id").context("nonempty item stack has no id")?;
    let id = vanilla_id(string(id, "item id")?)?;
    let id = match id.as_str() {
        "minecraft:chain" => "minecraft:iron_chain".to_owned(),
        "minecraft:grass" => "minecraft:short_grass".to_owned(),
        "minecraft:grass_path" => "minecraft:dirt_path".to_owned(),
        _ => id,
    };
    if id == "minecraft:air" {
        return Ok(());
    }
    ensure!(
        crate::materials::is_vanilla_item(&id),
        "unknown vanilla item {id:?}"
    );
    let count = count
        .checked_mul(multiplier)
        .context("nested item count overflow")?;
    increment(counts, &item_label(&id, data)?, count)?;

    if let Some(components) = data.get("components") {
        let components = as_compound(components, "item components")?;
        if let Some(items) = component(components, "container") {
            add_container_slots(counts, items, count, depth + 1)?;
        } else if let Some(contents) = component(components, "block_entity_data") {
            add_block_entity_scaled(counts, contents, count, depth + 1)?;
        }
        if let Some(items) = component(components, "bundle_contents") {
            add_stacks(counts, items, count, depth + 1)
                .context("invalid minecraft:bundle_contents contents")?;
        }
        if let Some(items) = component(components, "charged_projectiles") {
            add_projectiles(counts, items, count, depth + 1, false)
                .context("invalid minecraft:charged_projectiles contents")?;
        }
    } else if let Some(tag) = data.get("tag") {
        let tag = as_compound(tag, "item tag")?;
        if let Some(contents) = tag.get("BlockEntityTag") {
            add_block_entity_scaled(counts, contents, count, depth + 1)?;
        }
        if let Some(items) = tag.get("Items") {
            add_stacks(counts, items, count, depth + 1).context("invalid Items contents")?;
        }
        if let Some(items) = tag.get("ChargedProjectiles") {
            add_projectiles(counts, items, count, depth + 1, legacy_multishot(tag))
                .context("invalid ChargedProjectiles contents")?;
        }
    }
    Ok(())
}

fn add_stacks(
    counts: &mut BTreeMap<String, u64>,
    value: &Value,
    multiplier: u64,
    depth: usize,
) -> Result<()> {
    let Value::List(stacks) = value else {
        bail!("inventory contents must be a list");
    };
    for (index, stack) in stacks.iter().enumerate() {
        add_stack(counts, stack, multiplier, depth)
            .with_context(|| format!("inventory entry {index}"))?;
    }
    Ok(())
}

fn add_container_slots(
    counts: &mut BTreeMap<String, u64>,
    value: &Value,
    multiplier: u64,
    depth: usize,
) -> Result<()> {
    let Value::List(slots) = value else {
        bail!("minecraft:container contents must be a list of slots");
    };
    for (index, slot) in slots.iter().enumerate() {
        let slot = as_compound(slot, "container slot")?;
        let item = slot.get("item").context("container slot has no item")?;
        add_stack(counts, item, multiplier, depth)
            .with_context(|| format!("container slot {index}"))?;
    }
    Ok(())
}

fn add_projectiles(
    counts: &mut BTreeMap<String, u64>,
    value: &Value,
    multiplier: u64,
    depth: usize,
    legacy_multishot: bool,
) -> Result<()> {
    let Value::List(projectiles) = value else {
        bail!("charged projectiles must be a list");
    };
    for (index, projectile) in projectiles.iter().enumerate() {
        // Legacy Multishot duplicates ammunition without marking its phantom shots.
        if legacy_multishot && index > 0 {
            continue;
        }
        let data = as_compound(projectile, "charged projectile")?;
        if let Some(Value::Compound(components)) = data.get("components")
            && component(components, "intangible_projectile").is_some()
        {
            continue;
        }
        add_stack(counts, projectile, multiplier, depth)
            .with_context(|| format!("charged projectile {index}"))?;
    }
    Ok(())
}

fn legacy_multishot(tag: &Compound) -> bool {
    let Some(Value::List(enchantments)) = tag.get("Enchantments") else {
        return false;
    };
    enchantments.iter().any(|value| {
        let Value::Compound(enchantment) = value else {
            return false;
        };
        matches!(enchantment.get("id"), Some(Value::String(id)) if id == "minecraft:multishot" || id == "multishot")
            && enchantment.get("lvl").is_some_and(|level| {
                nonnegative_integer(level, "Multishot level").is_ok_and(|level| level > 0)
            })
    })
}

fn add_entity_at_depth(
    counts: &mut BTreeMap<String, u64>,
    value: &Value,
    include_contents: bool,
    depth: usize,
) -> Result<()> {
    let data = payload(value, depth)?;
    let id = data.get("id").context("entity has no id")?;
    let id = vanilla_id(string(id, "entity id")?)?;
    let name = id.strip_prefix("minecraft:").expect("validated vanilla id");
    match name {
        "item_frame"
        | "glow_item_frame"
        | "armor_stand"
        | "painting"
        | "minecart"
        | "chest_minecart"
        | "hopper_minecart"
        | "furnace_minecart"
        | "tnt_minecart"
        | "command_block_minecart"
        | "end_crystal" => increment(counts, &id, 1)?,
        "boat" | "chest_boat" => increment(counts, &boat_item(data, name == "chest_boat")?, 1)?,
        "bamboo_raft" | "bamboo_chest_raft" => increment(counts, &id, 1)?,
        name if name.ends_with("_boat") => {
            let wood = name
                .strip_suffix("_chest_boat")
                .or_else(|| name.strip_suffix("_boat"));
            ensure!(
                wood.is_some_and(is_boat_wood),
                "unsupported vanilla boat entity {id}"
            );
            increment(counts, &id, 1)?;
        }
        "cushion" => {
            let color = cushion_color(data)?;
            increment(counts, &format!("minecraft:{color}_cushion"), 1)?;
        }
        "item" => {}
        name if is_transient_or_command_entity(name) => {}
        _ => {
            let egg = format!("minecraft:{name}_spawn_egg");
            ensure!(
                crate::materials::is_vanilla_item(&egg),
                "entity {id} has no supported placeable material; use --exclude-entities and recreate it separately"
            );
            increment(counts, &egg, 1)?;
        }
    }
    if include_contents && !is_transient_or_command_entity(name) {
        add_entity_contents(counts, data, depth)?;
    }
    if let Some(passengers) = field(data, &["Passengers", "passengers"]) {
        let Value::List(passengers) = passengers else {
            bail!("entity passengers must be a list");
        };
        for (index, passenger) in passengers.iter().enumerate() {
            add_entity_at_depth(counts, passenger, include_contents, depth + 1)
                .with_context(|| format!("passenger {index}"))?;
        }
    }
    Ok(())
}

fn add_entity_contents(
    counts: &mut BTreeMap<String, u64>,
    data: &Compound,
    depth: usize,
) -> Result<()> {
    ensure!(
        !data.contains_key("LootTable") && !data.contains_key("loot_table"),
        "entity inventory contents are unresolved: generate its loot table before saving or use --exclude-contents"
    );
    add_inventory(counts, data, 1, depth)?;
    if let Some(item) = field(data, &["Item", "item"]) {
        add_stack(counts, item, 1, depth + 1)?;
    }
    if let Some(equipment) = data.get("equipment") {
        for (slot, item) in as_compound(equipment, "entity equipment")? {
            add_stack(counts, item, 1, depth + 1)
                .with_context(|| format!("entity equipment slot {slot:?}"))?;
        }
    } else {
        for names in [["ArmorItems", "armor_items"], ["HandItems", "hand_items"]] {
            if let Some(items) = field(data, &names) {
                add_stacks(counts, items, 1, depth + 1)?;
            }
        }
        for name in ["body_armor_item", "SaddleItem", "ArmorItem", "DecorItem"] {
            if let Some(item) = data.get(name) {
                add_stack(counts, item, 1, depth + 1)?;
            }
        }
        if !data.contains_key("SaddleItem")
            && let Some(saddle) = data.get("Saddle")
            && nonnegative_integer(saddle, "Saddle")? != 0
        {
            increment(counts, "minecraft:saddle", 1)?;
        }
    }
    if let Some(chested) = data.get("ChestedHorse")
        && nonnegative_integer(chested, "ChestedHorse")? != 0
    {
        increment(counts, "minecraft:chest", 1)?;
    }
    if data.contains_key("Leash") || data.contains_key("leash") {
        increment(counts, "minecraft:lead", 1)?;
    }
    Ok(())
}

fn boat_item(data: &Compound, chest: bool) -> Result<String> {
    let wood = field(data, &["Type", "type"])
        .map(|value| string(value, "boat type"))
        .transpose()?
        .unwrap_or("oak");
    ensure!(
        is_boat_wood(wood) || wood == "bamboo",
        "unsupported vanilla boat type {wood:?}"
    );
    let suffix = if wood == "bamboo" { "raft" } else { "boat" };
    let chest = if chest { "chest_" } else { "" };
    Ok(format!("minecraft:{wood}_{chest}{suffix}"))
}

fn is_boat_wood(wood: &str) -> bool {
    matches!(
        wood,
        "oak"
            | "spruce"
            | "birch"
            | "jungle"
            | "acacia"
            | "dark_oak"
            | "mangrove"
            | "cherry"
            | "pale_oak"
            | "poplar"
    )
}

fn cushion_color(data: &Compound) -> Result<&str> {
    let color = data
        .get("minecraft:cushion/color")
        .or_else(|| field(data, &["color", "Color"]))
        .or_else(|| {
            let Value::Compound(components) = data.get("components")? else {
                return None;
            };
            component(components, "cushion/color")
        });
    const COLORS: [&str; 16] = [
        "white",
        "orange",
        "magenta",
        "light_blue",
        "yellow",
        "lime",
        "pink",
        "gray",
        "light_gray",
        "cyan",
        "purple",
        "blue",
        "brown",
        "green",
        "red",
        "black",
    ];
    let color = match color {
        Some(Value::String(value)) => value.as_str(),
        Some(value) => {
            let index = nonnegative_integer(value, "cushion color")?;
            *COLORS
                .get(usize::try_from(index).context("invalid cushion color")?)
                .context("cushion color must be in 0..=15")?
        }
        None => "white",
    };
    ensure!(
        matches!(
            color,
            "white"
                | "orange"
                | "magenta"
                | "light_blue"
                | "yellow"
                | "lime"
                | "pink"
                | "gray"
                | "light_gray"
                | "cyan"
                | "purple"
                | "blue"
                | "brown"
                | "green"
                | "red"
                | "black"
        ),
        "invalid cushion color {color:?}"
    );
    Ok(color)
}

fn is_transient_or_command_entity(name: &str) -> bool {
    matches!(
        name,
        "leash_knot"
            | "spawner_minecart"
            | "area_effect_cloud"
            | "arrow"
            | "spectral_arrow"
            | "trident"
            | "firework_rocket"
            | "ender_pearl"
            | "egg"
            | "snowball"
            | "experience_bottle"
            | "experience_orb"
            | "potion"
            | "splash_potion"
            | "lingering_potion"
            | "evoker_fangs"
            | "fishing_bobber"
            | "lightning_bolt"
            | "marker"
            | "interaction"
            | "block_display"
            | "item_display"
            | "text_display"
            | "falling_block"
            | "tnt"
            | "fireball"
            | "small_fireball"
            | "dragon_fireball"
            | "wither_skull"
            | "shulker_bullet"
            | "wind_charge"
            | "breeze_wind_charge"
            | "eye_of_ender"
            | "player"
    )
}

fn payload(value: &Value, depth: usize) -> Result<&Compound> {
    ensure!(
        depth <= MAX_NESTING,
        "entity data exceeds {MAX_NESTING} nesting levels"
    );
    let data = as_compound(value, "entity data")?;
    if let Some(inner) = data.get("nbt") {
        as_compound(inner, "wrapped entity nbt")
    } else {
        Ok(data)
    }
}

fn as_compound<'a>(value: &'a Value, name: &str) -> Result<&'a Compound> {
    let Value::Compound(compound) = value else {
        bail!("{name} must be a compound");
    };
    Ok(compound)
}

fn field<'a>(data: &'a Compound, names: &[&str]) -> Option<&'a Value> {
    names.iter().find_map(|name| data.get(*name))
}

fn component<'a>(data: &'a Compound, name: &str) -> Option<&'a Value> {
    data.get(&format!("minecraft:{name}"))
        .or_else(|| data.get(name))
}

fn item_label(id: &str, data: &Compound) -> Result<String> {
    let mut details = BTreeMap::<String, String>::new();
    if let Some(components) = data.get("components") {
        for (name, value) in as_compound(components, "item components")? {
            let name = name.strip_prefix("minecraft:").unwrap_or(name);
            match name {
                "container" | "bundle_contents" | "charged_projectiles" => {}
                "potion_contents" => {
                    if let Value::String(potion) = value {
                        details.insert("potion".into(), vanilla_id(potion)?);
                        continue;
                    }
                    let potion = as_compound(value, "potion contents")?;
                    let mut remaining = potion.clone();
                    if let Some(potion) = remaining.remove("potion") {
                        details.insert(
                            "potion".into(),
                            vanilla_id(string(&potion, "potion type")?)?,
                        );
                    }
                    if !remaining.is_empty() {
                        details.insert(name.into(), snbt(&Value::Compound(remaining), 0)?);
                    }
                }
                "enchantments" | "stored_enchantments" => {
                    let enchantments = as_compound(value, "enchantments")?;
                    let value = enchantments.get("levels").unwrap_or(value);
                    details.insert(name.into(), snbt(value, 0)?);
                }
                "block_entity_data" => {
                    let value = content_free_block_entity(value)?;
                    if let Some(value) = value {
                        details.insert(name.into(), snbt(&value, 0)?);
                    }
                }
                _ => {
                    details.insert(name.into(), snbt(value, 0)?);
                }
            }
        }
    } else if let Some(tag) = data.get("tag") {
        let mut tag = as_compound(tag, "item tag")?.clone();
        if let Some(potion) = tag.remove("Potion") {
            details.insert(
                "potion".into(),
                vanilla_id(string(&potion, "potion type")?)?,
            );
        }
        for (legacy_name, name) in [
            ("Enchantments", "enchantments"),
            ("StoredEnchantments", "stored_enchantments"),
        ] {
            if let Some(enchantments) = tag.remove(legacy_name) {
                let Value::List(enchantments) = enchantments else {
                    bail!("legacy enchantments must be a list");
                };
                let mut levels = Compound::new();
                for enchantment in enchantments {
                    let enchantment = as_compound(&enchantment, "enchantment")?;
                    let id = enchantment.get("id").context("enchantment has no id")?;
                    let id = vanilla_id(string(id, "enchantment id")?)?;
                    let level = enchantment.get("lvl").context("enchantment has no level")?;
                    let level = nonnegative_integer(level, "enchantment level")?;
                    levels.insert(
                        id,
                        Value::Int(i32::try_from(level).context("enchantment level is too large")?),
                    );
                }
                details.insert(name.into(), snbt(&Value::Compound(levels), 0)?);
            }
        }
        for (legacy_name, name) in [
            ("Damage", "damage"),
            ("RepairCost", "repair_cost"),
            ("map", "map_id"),
        ] {
            if let Some(value) = tag.remove(legacy_name) {
                details.insert(name.into(), nonnegative_integer(&value, name)?.to_string());
            }
        }
        if let Some(display) = tag.remove("display") {
            let mut display = as_compound(&display, "legacy display data")?.clone();
            for (legacy_name, name) in [
                ("Name", "custom_name"),
                ("Lore", "lore"),
                ("color", "dyed_color"),
            ] {
                if let Some(value) = display.remove(legacy_name) {
                    details.insert(name.into(), snbt(&value, 0)?);
                }
            }
            if !display.is_empty() {
                tag.insert("display".into(), Value::Compound(display));
            }
        }
        if let Some(value) = tag.remove("BlockEntityTag")
            && let Some(value) = content_free_block_entity(&value)?
        {
            details.insert("block_entity_data".into(), snbt(&value, 0)?);
        }
        tag.remove("Items");
        tag.remove("ChargedProjectiles");
        if !tag.is_empty() {
            details.insert("legacy_tag".into(), snbt(&Value::Compound(tag), 0)?);
        }
    }
    if details.is_empty() {
        Ok(id.to_owned())
    } else {
        Ok(format!(
            "{id}[{}]",
            details
                .into_iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join(",")
        ))
    }
}

fn content_free_block_entity(value: &Value) -> Result<Option<Value>> {
    let mut data = as_compound(value, "item block entity data")?.clone();
    for name in [
        "id",
        "x",
        "y",
        "z",
        "Items",
        "items",
        "Book",
        "book",
        "RecordItem",
        "record_item",
        "item",
        "Item",
    ] {
        data.remove(name);
    }
    Ok((!data.is_empty()).then_some(Value::Compound(data)))
}

fn snbt(value: &Value, depth: usize) -> Result<String> {
    ensure!(
        depth <= MAX_NESTING,
        "item metadata exceeds {MAX_NESTING} nesting levels"
    );
    Ok(match value {
        Value::Byte(value) => format!("{value}b"),
        Value::Short(value) => format!("{value}s"),
        Value::Int(value) => value.to_string(),
        Value::Long(value) => format!("{value}L"),
        Value::Float(value) => format!("{value}f"),
        Value::Double(value) => format!("{value}d"),
        Value::String(value) => quote(value),
        Value::ByteArray(values) => format!(
            "[B;{}]",
            values
                .iter()
                .map(|value| format!("{value}b"))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::IntArray(values) => format!(
            "[I;{}]",
            values
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::LongArray(values) => format!(
            "[L;{}]",
            values
                .iter()
                .map(|value| format!("{value}L"))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::List(values) => format!(
            "[{}]",
            values
                .iter()
                .map(|value| snbt(value, depth + 1))
                .collect::<Result<Vec<_>>>()?
                .join(",")
        ),
        Value::Compound(values) => {
            let sorted = values.iter().collect::<BTreeMap<_, _>>();
            format!(
                "{{{}}}",
                sorted
                    .into_iter()
                    .map(|(name, value)| Ok(format!("{}:{}", quote(name), snbt(value, depth + 1)?)))
                    .collect::<Result<Vec<_>>>()?
                    .join(",")
            )
        }
    })
}

fn quote(value: &str) -> String {
    let mut quoted = String::from("\"");
    for character in value.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            value if value.is_control() => quoted.push_str(&format!("\\u{:04x}", u32::from(value))),
            value => quoted.push(value),
        }
    }
    quoted.push('"');
    quoted
}

fn string<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    let Value::String(value) = value else {
        bail!("{name} must be a string");
    };
    Ok(value)
}

fn vanilla_id(id: &str) -> Result<String> {
    let id = if id.contains(':') {
        id.to_owned()
    } else {
        format!("minecraft:{id}")
    };
    let name = id
        .strip_prefix("minecraft:")
        .with_context(|| format!("non-vanilla item or entity {id:?} is unsupported"))?;
    ensure!(
        !name.is_empty()
            && name.bytes().all(|byte| byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'/' | b'.' | b'-')),
        "invalid vanilla item or entity id {id:?}"
    );
    Ok(id)
}

fn nonnegative_integer(value: &Value, name: &str) -> Result<u64> {
    let integer = match value {
        Value::Byte(value) => i64::from(*value),
        Value::Short(value) => i64::from(*value),
        Value::Int(value) => i64::from(*value),
        Value::Long(value) => *value,
        _ => bail!("{name} must be an integer"),
    };
    u64::try_from(integer).with_context(|| format!("{name} must not be negative"))
}

fn increment(counts: &mut BTreeMap<String, u64>, id: &str, count: u64) -> Result<()> {
    let total = counts.entry(id.to_owned()).or_default();
    *total = total
        .checked_add(count)
        .with_context(|| format!("material count overflow for {id}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compound(entries: &[(&str, Value)]) -> Value {
        Value::Compound(
            entries
                .iter()
                .map(|(key, value)| ((*key).into(), value.clone()))
                .collect(),
        )
    }

    fn legacy_stack(id: &str, count: i8) -> Value {
        compound(&[
            ("id", Value::String(id.into())),
            ("Count", Value::Byte(count)),
        ])
    }

    fn modern_stack(id: &str, count: i32) -> Value {
        compound(&[
            ("id", Value::String(id.into())),
            ("count", Value::Int(count)),
        ])
    }

    fn entity(id: &str, fields: &[(&str, Value)]) -> Value {
        let mut fields = fields.to_vec();
        fields.push(("id", Value::String(format!("minecraft:{id}"))));
        compound(&fields)
    }

    #[test]
    fn combines_chest_hopper_and_other_inventories() {
        let mut counts = BTreeMap::new();
        for id in [
            "chest",
            "hopper",
            "barrel",
            "furnace",
            "dispenser",
            "dropper",
            "crafter",
            "brewing_stand",
            "shulker_box",
            "chiseled_bookshelf",
            "campfire",
            "copper_chest",
        ] {
            add_block_entity(
                &mut counts,
                &entity(
                    id,
                    &[(
                        "Items",
                        Value::List(vec![
                            legacy_stack("minecraft:stone", 64),
                            modern_stack("diamond", 2),
                            compound(&[]),
                        ]),
                    )],
                ),
            )
            .unwrap();
        }
        assert_eq!(counts["minecraft:stone"], 768);
        assert_eq!(counts["minecraft:diamond"], 24);
    }

    #[test]
    fn counts_books_records_and_decorated_pot_storage() {
        let mut counts = BTreeMap::new();
        for (id, key, item) in [
            ("lectern", "Book", "written_book"),
            ("jukebox", "RecordItem", "music_disc_cat"),
            ("decorated_pot", "item", "emerald"),
        ] {
            add_block_entity(&mut counts, &entity(id, &[(key, modern_stack(item, 1))])).unwrap();
        }
        assert_eq!(counts.len(), 3);
        assert_eq!(counts["minecraft:emerald"], 1);
    }

    #[test]
    fn recursively_counts_legacy_shulker_boxes_and_bundles() {
        let mut bundle = legacy_stack("minecraft:bundle", 2);
        let Value::Compound(bundle_data) = &mut bundle else {
            unreachable!()
        };
        bundle_data.insert(
            "tag".into(),
            compound(&[(
                "Items",
                Value::List(vec![legacy_stack("minecraft:diamond", 3)]),
            )]),
        );
        let mut shulker = legacy_stack("minecraft:red_shulker_box", 1);
        let Value::Compound(shulker_data) = &mut shulker else {
            unreachable!()
        };
        shulker_data.insert(
            "tag".into(),
            compound(&[(
                "BlockEntityTag",
                compound(&[("Items", Value::List(vec![bundle]))]),
            )]),
        );
        let mut counts = BTreeMap::new();
        add_item_stack(&mut counts, &shulker).unwrap();
        assert_eq!(counts["minecraft:red_shulker_box"], 1);
        assert_eq!(counts["minecraft:bundle"], 2);
        assert_eq!(counts["minecraft:diamond"], 6);
    }

    #[test]
    fn recursively_counts_modern_container_slots_and_bundles() {
        let mut bundle = modern_stack("bundle", 1);
        let Value::Compound(bundle_data) = &mut bundle else {
            unreachable!()
        };
        bundle_data.insert(
            "components".into(),
            compound(&[(
                "bundle_contents",
                Value::List(vec![modern_stack("gold_ingot", 9)]),
            )]),
        );
        let mut shulker = modern_stack("blue_shulker_box", 2);
        let Value::Compound(shulker_data) = &mut shulker else {
            unreachable!()
        };
        shulker_data.insert(
            "components".into(),
            compound(&[(
                "minecraft:container",
                Value::List(vec![
                    compound(&[("slot", Value::Int(0)), ("item", bundle)]),
                    compound(&[("slot", Value::Int(1)), ("item", modern_stack("stone", 64))]),
                ]),
            )]),
        );
        let mut counts = BTreeMap::new();
        add_item_stack(&mut counts, &shulker).unwrap();
        assert_eq!(counts["minecraft:blue_shulker_box"], 2);
        assert_eq!(counts["minecraft:bundle"], 2);
        assert_eq!(counts["minecraft:gold_ingot"], 18);
        assert_eq!(counts["minecraft:stone"], 128);
    }

    #[test]
    fn counts_frames_paintings_crystals_and_displayed_items() {
        let mut counts = BTreeMap::new();
        for id in ["item_frame", "glow_item_frame"] {
            add_entity(
                &mut counts,
                &entity(id, &[("Item", modern_stack("filled_map", 1))]),
            )
            .unwrap();
        }
        for id in ["painting", "end_crystal"] {
            add_entity(&mut counts, &entity(id, &[])).unwrap();
            assert_eq!(counts[&format!("minecraft:{id}")], 1);
        }
        assert_eq!(counts["minecraft:item_frame"], 1);
        assert_eq!(counts["minecraft:glow_item_frame"], 1);
        assert_eq!(counts["minecraft:filled_map"], 2);
    }

    #[test]
    fn counts_legacy_and_modern_armor_stand_equipment() {
        let mut counts = BTreeMap::new();
        let legacy = entity(
            "armor_stand",
            &[
                (
                    "ArmorItems",
                    Value::List(vec![
                        compound(&[]),
                        legacy_stack("minecraft:diamond_leggings", 1),
                    ]),
                ),
                (
                    "HandItems",
                    Value::List(vec![
                        legacy_stack("minecraft:diamond_sword", 1),
                        compound(&[]),
                    ]),
                ),
            ],
        );
        let modern = entity(
            "armor_stand",
            &[(
                "equipment",
                compound(&[
                    ("head", modern_stack("carved_pumpkin", 1)),
                    ("mainhand", modern_stack("diamond_sword", 1)),
                ]),
            )],
        );
        add_entity(&mut counts, &legacy).unwrap();
        add_entity(&mut counts, &modern).unwrap();
        assert_eq!(counts["minecraft:armor_stand"], 2);
        assert_eq!(counts["minecraft:diamond_leggings"], 1);
        assert_eq!(counts["minecraft:diamond_sword"], 2);
        assert_eq!(counts["minecraft:carved_pumpkin"], 1);
    }

    #[test]
    fn counts_all_placeable_minecarts_and_their_contents() {
        let mut counts = BTreeMap::new();
        for id in [
            "minecart",
            "chest_minecart",
            "hopper_minecart",
            "furnace_minecart",
            "tnt_minecart",
            "command_block_minecart",
        ] {
            add_entity(&mut counts, &entity(id, &[])).unwrap();
            assert_eq!(counts[&format!("minecraft:{id}")], 1);
        }
        add_entity(
            &mut counts,
            &entity(
                "chest_minecart",
                &[("Items", Value::List(vec![modern_stack("iron_ingot", 32)]))],
            ),
        )
        .unwrap();
        assert_eq!(counts["minecraft:chest_minecart"], 2);
        assert_eq!(counts["minecraft:iron_ingot"], 32);
    }

    #[test]
    fn counts_boat_variants_and_passengers() {
        let mut counts = BTreeMap::new();
        add_entity(
            &mut counts,
            &entity("boat", &[("Type", Value::String("dark_oak".into()))]),
        )
        .unwrap();
        add_entity(
            &mut counts,
            &entity(
                "chest_boat",
                &[
                    ("Type", Value::String("bamboo".into())),
                    ("Items", Value::List(vec![modern_stack("coal", 7)])),
                ],
            ),
        )
        .unwrap();
        add_entity(
            &mut counts,
            &entity(
                "pale_oak_chest_boat",
                &[("Passengers", Value::List(vec![entity("armor_stand", &[])]))],
            ),
        )
        .unwrap();
        assert_eq!(counts["minecraft:dark_oak_boat"], 1);
        assert_eq!(counts["minecraft:bamboo_chest_raft"], 1);
        assert_eq!(counts["minecraft:pale_oak_chest_boat"], 1);
        assert_eq!(counts["minecraft:armor_stand"], 1);
        assert_eq!(counts["minecraft:coal"], 7);
    }

    #[test]
    fn handles_wrapped_payloads_empty_stacks_and_default_count() {
        let mut counts = BTreeMap::new();
        add_entity(
            &mut counts,
            &compound(&[(
                "nbt",
                entity(
                    "item",
                    &[("Item", compound(&[("id", Value::String("stone".into()))]))],
                ),
            )]),
        )
        .unwrap();
        add_block_entity(
            &mut counts,
            &compound(&[(
                "nbt",
                entity(
                    "hopper",
                    &[(
                        "items",
                        Value::List(vec![
                            modern_stack("air", 1),
                            modern_stack("diamond", 0),
                            compound(&[]),
                        ]),
                    )],
                ),
            )]),
        )
        .unwrap();
        assert_eq!(counts, BTreeMap::from([("minecraft:stone".into(), 1)]));
    }

    #[test]
    fn rejects_missing_malformed_negative_or_overflowing_stacks() {
        for stack in [
            compound(&[("Count", Value::Byte(1))]),
            compound(&[("id", Value::Int(1))]),
            modern_stack("stone", -1),
            modern_stack("mod:stone", 1),
        ] {
            assert!(add_item_stack(&mut BTreeMap::new(), &stack).is_err());
        }
        let mut counts = BTreeMap::from([("minecraft:stone".into(), u64::MAX)]);
        assert!(add_item_stack(&mut counts, &modern_stack("stone", 1)).is_err());
    }

    #[test]
    fn rejects_unresolved_loot_and_unknown_entities() {
        assert!(
            add_block_entity(
                &mut BTreeMap::new(),
                &entity(
                    "chest",
                    &[(
                        "LootTable",
                        Value::String("minecraft:chests/simple_dungeon".into())
                    ),]
                )
            )
            .is_err()
        );
        for id in ["unknown_entity", "giant"] {
            let message = add_entity(&mut BTreeMap::new(), &entity(id, &[]))
                .unwrap_err()
                .to_string();
            assert!(message.contains(id));
        }
    }

    #[test]
    fn counts_cushion_colors() {
        let mut counts = BTreeMap::new();
        add_entity(
            &mut counts,
            &entity(
                "cushion",
                &[("minecraft:cushion/color", Value::String("red".into()))],
            ),
        )
        .unwrap();
        assert_eq!(counts["minecraft:red_cushion"], 1);
    }

    #[test]
    fn counts_copied_mobs_as_spawn_eggs_with_saved_equipment() {
        let mut counts = BTreeMap::new();
        add_entity(
            &mut counts,
            &entity(
                "zombie",
                &[(
                    "equipment",
                    compound(&[("mainhand", modern_stack("iron_sword", 1))]),
                )],
            ),
        )
        .unwrap();
        add_entity(
            &mut counts,
            &entity(
                "villager",
                &[("Inventory", Value::List(vec![modern_stack("bread", 12)]))],
            ),
        )
        .unwrap();
        assert_eq!(counts["minecraft:zombie_spawn_egg"], 1);
        assert_eq!(counts["minecraft:iron_sword"], 1);
        assert_eq!(counts["minecraft:villager_spawn_egg"], 1);
        assert_eq!(counts["minecraft:bread"], 12);
    }

    #[test]
    fn skips_command_only_and_transient_entities() {
        let mut counts = BTreeMap::new();
        for id in [
            "leash_knot",
            "block_display",
            "spawner_minecart",
            "arrow",
            "item_display",
        ] {
            add_entity(
                &mut counts,
                &entity(id, &[("item", modern_stack("stone", 1))]),
            )
            .unwrap();
        }
        assert!(counts.is_empty());
    }

    #[test]
    fn excluding_contents_keeps_entities_and_passengers() {
        let mut counts = BTreeMap::new();
        let boat = entity(
            "oak_chest_boat",
            &[
                ("Items", Value::List(vec![modern_stack("stone", 64)])),
                (
                    "Passengers",
                    Value::List(vec![entity(
                        "armor_stand",
                        &[(
                            "equipment",
                            compound(&[("head", modern_stack("diamond_helmet", 1))]),
                        )],
                    )]),
                ),
            ],
        );
        add_entity_with_contents(&mut counts, &boat, false).unwrap();
        add_entity_with_contents(
            &mut counts,
            &entity("item_frame", &[("Item", modern_stack("diamond", 1))]),
            false,
        )
        .unwrap();
        assert_eq!(
            counts,
            BTreeMap::from([
                ("minecraft:oak_chest_boat".into(), 1),
                ("minecraft:armor_stand".into(), 1),
                ("minecraft:item_frame".into(), 1),
            ])
        );
    }

    #[test]
    fn distinguishes_potion_types_and_matches_legacy_modern_labels() {
        let mut counts = BTreeMap::new();
        let mut legacy = legacy_stack("minecraft:potion", 2);
        let Value::Compound(legacy_data) = &mut legacy else {
            unreachable!()
        };
        legacy_data.insert(
            "tag".into(),
            compound(&[("Potion", Value::String("minecraft:water".into()))]),
        );
        let mut modern = modern_stack("potion", 3);
        let Value::Compound(modern_data) = &mut modern else {
            unreachable!()
        };
        modern_data.insert(
            "components".into(),
            compound(&[(
                "minecraft:potion_contents",
                compound(&[("potion", Value::String("minecraft:water".into()))]),
            )]),
        );
        add_item_stack(&mut counts, &legacy).unwrap();
        add_item_stack(&mut counts, &modern).unwrap();
        assert_eq!(counts["minecraft:potion[potion=minecraft:water]"], 5);
        let Value::Compound(modern_data) = &mut modern else {
            unreachable!()
        };
        modern_data.insert(
            "components".into(),
            compound(&[(
                "minecraft:potion_contents",
                compound(&[("potion", Value::String("minecraft:healing".into()))]),
            )]),
        );
        add_item_stack(&mut counts, &modern).unwrap();
        assert_eq!(counts["minecraft:potion[potion=minecraft:healing]"], 3);
        assert_eq!(counts.len(), 2);
    }

    #[test]
    fn preserves_enchanted_book_variants_with_sorted_metadata() {
        let mut counts = BTreeMap::new();
        let mut legacy = legacy_stack("minecraft:enchanted_book", 1);
        let Value::Compound(legacy_data) = &mut legacy else {
            unreachable!()
        };
        legacy_data.insert(
            "tag".into(),
            compound(&[(
                "StoredEnchantments",
                Value::List(vec![
                    compound(&[
                        ("id", Value::String("minecraft:mending".into())),
                        ("lvl", Value::Short(1)),
                    ]),
                    compound(&[
                        ("id", Value::String("minecraft:unbreaking".into())),
                        ("lvl", Value::Short(3)),
                    ]),
                ]),
            )]),
        );
        let mut modern = modern_stack("enchanted_book", 2);
        let Value::Compound(modern_data) = &mut modern else {
            unreachable!()
        };
        modern_data.insert(
            "components".into(),
            compound(&[(
                "minecraft:stored_enchantments",
                compound(&[
                    ("minecraft:unbreaking", Value::Int(3)),
                    ("minecraft:mending", Value::Int(1)),
                ]),
            )]),
        );
        add_item_stack(&mut counts, &legacy).unwrap();
        add_item_stack(&mut counts, &modern).unwrap();
        assert_eq!(
            counts["minecraft:enchanted_book[stored_enchantments={\"minecraft:mending\":1,\"minecraft:unbreaking\":3}]"],
            3
        );
    }

    #[test]
    fn rejects_excessive_nesting_and_nested_count_overflow() {
        let mut stack = modern_stack("diamond", 1);
        for _ in 0..70 {
            let mut bundle = modern_stack("bundle", 1);
            let Value::Compound(data) = &mut bundle else {
                unreachable!()
            };
            data.insert(
                "components".into(),
                compound(&[("bundle_contents", Value::List(vec![stack]))]),
            );
            stack = bundle;
        }
        assert!(
            format!(
                "{:#}",
                add_item_stack(&mut BTreeMap::new(), &stack).unwrap_err()
            )
            .contains("nesting")
        );

        let mut stack = modern_stack("diamond", i32::MAX);
        for _ in 0..2 {
            let mut bundle = modern_stack("bundle", i32::MAX);
            let Value::Compound(data) = &mut bundle else {
                unreachable!()
            };
            data.insert(
                "components".into(),
                compound(&[("bundle_contents", Value::List(vec![stack]))]),
            );
            stack = bundle;
        }
        assert!(
            format!(
                "{:#}",
                add_item_stack(&mut BTreeMap::new(), &stack).unwrap_err()
            )
            .contains("overflow")
        );
    }

    #[test]
    fn accepts_every_registered_vanilla_item_in_an_inventory() {
        let mut counts = BTreeMap::new();
        let names: Vec<&str> =
            serde_json::from_str(include_str!("../data/vanilla_items_26_3.json")).unwrap();
        for name in &names {
            add_item_stack(&mut counts, &modern_stack(name, 1)).unwrap();
        }
        assert_eq!(names.len(), 1658);
        for name in names {
            if name != "air" {
                assert_eq!(counts[&format!("minecraft:{name}")], 1, "{name}");
            }
        }
    }

    #[test]
    fn accepts_compact_potion_contents() {
        let mut stack = modern_stack("potion", 1);
        let Value::Compound(data) = &mut stack else {
            unreachable!()
        };
        data.insert(
            "components".into(),
            compound(&[("potion_contents", Value::String("water".into()))]),
        );
        let mut counts = BTreeMap::new();
        add_item_stack(&mut counts, &stack).unwrap();
        assert_eq!(counts["minecraft:potion[potion=minecraft:water]"], 1);
    }

    #[test]
    fn counts_loaded_ammunition_without_modern_multishot_phantoms() {
        let mut phantom = modern_stack("arrow", 1);
        let Value::Compound(phantom_data) = &mut phantom else {
            unreachable!()
        };
        phantom_data.insert(
            "components".into(),
            compound(&[("minecraft:intangible_projectile", compound(&[]))]),
        );
        let mut crossbow = modern_stack("crossbow", 2);
        let Value::Compound(crossbow_data) = &mut crossbow else {
            unreachable!()
        };
        crossbow_data.insert(
            "components".into(),
            compound(&[(
                "charged_projectiles",
                Value::List(vec![modern_stack("arrow", 1), phantom.clone(), phantom]),
            )]),
        );
        let mut counts = BTreeMap::new();
        add_item_stack(&mut counts, &crossbow).unwrap();
        assert_eq!(counts["minecraft:crossbow"], 2);
        assert_eq!(counts["minecraft:arrow"], 2);
        assert_eq!(counts.len(), 2);

        let Value::Compound(crossbow_data) = &mut crossbow else {
            unreachable!()
        };
        crossbow_data.insert(
            "components".into(),
            compound(&[(
                "charged_projectiles",
                Value::List(vec![modern_stack("arrow", 2), modern_stack("arrow", 3)]),
            )]),
        );
        add_item_stack(&mut counts, &crossbow).unwrap();
        assert_eq!(counts["minecraft:crossbow"], 4);
        assert_eq!(counts["minecraft:arrow"], 12);
    }

    #[test]
    fn counts_one_loaded_projectile_for_legacy_multishot() {
        let mut crossbow = legacy_stack("minecraft:crossbow", 1);
        let Value::Compound(crossbow_data) = &mut crossbow else {
            unreachable!()
        };
        crossbow_data.insert(
            "tag".into(),
            compound(&[
                (
                    "Enchantments",
                    Value::List(vec![compound(&[
                        ("id", Value::String("minecraft:multishot".into())),
                        ("lvl", Value::Short(1)),
                    ])]),
                ),
                (
                    "ChargedProjectiles",
                    Value::List(vec![legacy_stack("minecraft:arrow", 1); 3]),
                ),
            ]),
        );
        let mut counts = BTreeMap::new();
        add_item_stack(&mut counts, &crossbow).unwrap();
        assert_eq!(counts["minecraft:arrow"], 1);
        assert_eq!(counts.values().sum::<u64>(), 2);

        add_item_stack(&mut counts, &legacy_stack("minecraft:arrow", 64)).unwrap();
        assert_eq!(counts["minecraft:arrow"], 65);
    }
}
