use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use fastnbt::{LongArray, Value};
use flate2::read::GzDecoder;
use serde::de::{Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(test, derive(serde::Serialize))]
pub struct BlockState {
    #[cfg_attr(test, serde(rename = "Name"))]
    pub name: String,
    #[cfg_attr(test, serde(rename = "Properties"))]
    pub properties: BTreeMap<String, String>,
}

impl<'de> Deserialize<'de> for BlockState {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct BlockStateVisitor;

        impl<'de> Visitor<'de> for BlockStateVisitor {
            type Value = BlockState;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a block state compound or block ID string")
            }

            fn visit_str<E>(self, name: &str) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                self.visit_string(name.into())
            }

            fn visit_string<E>(self, name: String) -> std::result::Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(BlockState {
                    name,
                    properties: BTreeMap::new(),
                })
            }

            fn visit_map<M>(self, mut map: M) -> std::result::Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut name = None;
                let mut properties = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "Name" | "id" => {
                            if name.is_some() {
                                return Err(M::Error::duplicate_field("Name/id"));
                            }
                            name =
                                Some(map.next_value::<String>().map_err(|error| {
                                    M::Error::custom(format!("{key}: {error}"))
                                })?);
                        }
                        "Properties" | "properties" => {
                            if properties.is_some() {
                                return Err(M::Error::duplicate_field("Properties/properties"));
                            }
                            properties = Some(
                                map.next_value::<BTreeMap<String, String>>()
                                    .map_err(|error| M::Error::custom(format!("{key}: {error}")))?,
                            );
                        }
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(BlockState {
                    name: name.ok_or_else(|| M::Error::missing_field("Name or id"))?,
                    properties: properties.unwrap_or_default(),
                })
            }
        }

        deserializer.deserialize_any(BlockStateVisitor)
    }
}

pub type StateCounts = BTreeMap<BlockState, u64>;

#[derive(Debug, Default)]
pub struct SchematicData {
    pub states: StateCounts,
    pub block_entities: Vec<Value>,
    pub entities: Vec<Value>,
}

const MAX_SCHEMATIC_VERSION: i32 = 7;

#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
struct Schematic {
    #[serde(rename = "Version")]
    version: i32,
    #[serde(rename = "Regions", deserialize_with = "deserialize_regions")]
    regions: BTreeMap<String, Region>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
struct Region {
    #[serde(rename = "Size")]
    size: Size,
    #[serde(rename = "BlockStatePalette", deserialize_with = "deserialize_palette")]
    palette: Vec<BlockState>,
    #[serde(rename = "BlockStates")]
    block_states: LongArray,
    #[serde(rename = "TileEntities", default)]
    block_entities: Vec<Value>,
    #[serde(rename = "Entities", default)]
    entities: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
struct Size {
    x: i32,
    y: i32,
    z: i32,
}

fn deserialize_regions<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, Region>, D::Error>
where
    D: Deserializer<'de>,
{
    struct RegionsVisitor;

    impl<'de> Visitor<'de> for RegionsVisitor {
        type Value = BTreeMap<String, Region>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a compound containing named schematic regions")
        }

        fn visit_map<M>(self, mut map: M) -> std::result::Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut regions = BTreeMap::new();
            while let Some(name) = map.next_key::<String>()? {
                let region = map
                    .next_value::<Region>()
                    .map_err(|error| M::Error::custom(format!("region {name:?}: {error}")))?;
                if regions.insert(name.clone(), region).is_some() {
                    return Err(M::Error::custom(format!("duplicate region {name:?}")));
                }
            }
            Ok(regions)
        }
    }

    deserializer.deserialize_map(RegionsVisitor)
}

fn deserialize_palette<'de, D>(deserializer: D) -> std::result::Result<Vec<BlockState>, D::Error>
where
    D: Deserializer<'de>,
{
    struct PaletteVisitor;

    impl<'de> Visitor<'de> for PaletteVisitor {
        type Value = Vec<BlockState>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a list of block states")
        }

        fn visit_seq<S>(self, mut sequence: S) -> std::result::Result<Self::Value, S::Error>
        where
            S: SeqAccess<'de>,
        {
            let mut palette = Vec::new();
            loop {
                let index = palette.len();
                let state = sequence
                    .next_element::<BlockState>()
                    .map_err(|error| S::Error::custom(format!("palette entry {index}: {error}")))?;
                let Some(state) = state else {
                    break;
                };
                palette.push(state);
            }
            Ok(palette)
        }
    }

    deserializer.deserialize_seq(PaletteVisitor)
}

pub fn read_litematic(path: &Path) -> Result<StateCounts> {
    Ok(read_schematic(path)?.states)
}

pub fn read_schematic(path: &Path) -> Result<SchematicData> {
    let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    read_gzip_data(BufReader::new(file))
        .with_context(|| format!("cannot read litematic {}", path.display()))
}

#[cfg(test)]
fn read_gzip<R: Read>(reader: R) -> Result<StateCounts> {
    Ok(read_gzip_data(reader)?.states)
}

fn read_gzip_data<R: Read>(reader: R) -> Result<SchematicData> {
    let mut reader = BufReader::new(GzDecoder::new(reader));
    let schematic: Schematic =
        fastnbt::from_reader(&mut reader).context("invalid gzip-compressed Java NBT schematic")?;

    // Read through the gzip trailer to validate truncation and the checksum.
    let mut trailing_byte = [0u8; 1];
    ensure!(
        reader
            .read(&mut trailing_byte)
            .context("invalid or truncated gzip stream")?
            == 0,
        "unexpected data after the NBT root compound"
    );

    decode_schematic(schematic)
}

#[cfg(test)]
fn count_schematic(schematic: Schematic) -> Result<StateCounts> {
    Ok(decode_schematic(schematic)?.states)
}

fn decode_schematic(schematic: Schematic) -> Result<SchematicData> {
    ensure!(
        (1..=MAX_SCHEMATIC_VERSION).contains(&schematic.version),
        "unsupported Litematic Version {}; supported versions are 1 through {}",
        schematic.version,
        MAX_SCHEMATIC_VERSION
    );
    ensure!(
        !schematic.regions.is_empty(),
        "schematic contains no regions"
    );

    let mut data = SchematicData::default();
    for (name, mut region) in schematic.regions {
        let legacy = schematic.version == 1;
        let block_entities = normalize_payloads(
            std::mem::take(&mut region.block_entities),
            legacy.then_some("TileNBT"),
            "TileEntities",
        )
        .with_context(|| format!("region {name:?}"))?;
        let entities = normalize_payloads(
            std::mem::take(&mut region.entities),
            legacy.then_some("EntityData"),
            "Entities",
        )
        .with_context(|| format!("region {name:?}"))?;
        count_region(region, &mut data.states).with_context(|| format!("region {name:?}"))?;
        data.block_entities.extend(block_entities);
        data.entities.extend(entities);
    }
    Ok(data)
}

fn normalize_payloads(
    payloads: Vec<Value>,
    legacy_key: Option<&str>,
    field: &str,
) -> Result<Vec<Value>> {
    payloads
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            ensure!(
                matches!(value, Value::Compound(_)),
                "{field} entry {index} must be a compound"
            );
            if let Some(key) = legacy_key {
                let Value::Compound(mut wrapper) = value else {
                    unreachable!();
                };
                let payload = wrapper
                    .remove(key)
                    .with_context(|| format!("{field} entry {index} is missing {key}"))?;
                ensure!(
                    matches!(payload, Value::Compound(_)),
                    "{field} entry {index}.{key} must be a compound"
                );
                Ok(payload)
            } else {
                Ok(value)
            }
        })
        .collect()
}

fn count_region(region: Region, counts: &mut StateCounts) -> Result<()> {
    ensure!(!region.palette.is_empty(), "BlockStatePalette is empty");
    for (index, state) in region.palette.iter().enumerate() {
        ensure!(
            !state.name.trim().is_empty(),
            "palette entry {index} has an empty Name"
        );
        ensure!(
            state.properties.keys().all(|key| !key.is_empty()),
            "palette entry {index} has an empty property name"
        );
    }

    let [size_x, size_y, size_z] = region.size.dimensions()?;
    let volume = size_x
        .checked_mul(size_y)
        .and_then(|layer| layer.checked_mul(size_z))
        .context("Size volume overflows the supported address range")?;
    // ceil(log2(n)) uses integer arithmetic, including n=1 and powers of two.
    let bits = (usize::BITS - (region.palette.len() - 1).leading_zeros()).max(2);
    let total_bits = volume
        .checked_mul(bits as usize)
        .context("packed block data size overflows the supported address range")?;
    let required_longs = total_bits.div_ceil(64);
    ensure!(
        region.block_states.len() >= required_longs,
        "BlockStates is truncated: {volume} blocks at {bits} bits each require {required_longs} longs, found {}",
        region.block_states.len()
    );

    let mut histogram = vec![0u64; region.palette.len()];
    let mask = (1u64 << bits) - 1;
    for block_index in 0..volume {
        let bit_index = block_index * bits as usize;
        let word_index = bit_index / 64;
        let offset = bit_index % 64;
        // Java NBT stores signed longs; cast before shifting to retain all bits.
        let mut palette_index = (region.block_states[word_index] as u64) >> offset;
        if offset + bits as usize > 64 {
            palette_index |= (region.block_states[word_index + 1] as u64) << (64 - offset);
        }
        let palette_index = (palette_index & mask) as usize;
        let Some(count) = histogram.get_mut(palette_index) else {
            let x = block_index % size_x;
            let y = block_index / (size_x * size_z);
            let z = (block_index / size_x) % size_z;
            anyhow::bail!(
                "block {block_index} (x={x}, y={y}, z={z}) references invalid palette index {palette_index}; palette contains {} entries",
                region.palette.len()
            );
        };
        *count = count.checked_add(1).context("block count overflows u64")?;
    }

    for (state, count) in region.palette.into_iter().zip(histogram) {
        if count > 0 {
            let total = counts.entry(state).or_default();
            *total = total
                .checked_add(count)
                .context("combined block state count overflows u64")?;
        }
    }
    Ok(())
}

impl Size {
    fn dimensions(&self) -> Result<[usize; 3]> {
        let mut dimensions = [0; 3];
        for (index, (axis, value)) in [("x", self.x), ("y", self.y), ("z", self.z)]
            .into_iter()
            .enumerate()
        {
            ensure!(value != 0, "Size.{axis} must be nonzero");
            dimensions[index] = value
                .checked_abs()
                .with_context(|| format!("Size.{axis} absolute value overflows i32"))?
                as usize;
        }
        Ok(dimensions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    fn state(name: &str) -> BlockState {
        BlockState {
            name: name.into(),
            properties: BTreeMap::new(),
        }
    }

    fn packed(indices: &[usize], bits: usize) -> LongArray {
        let mut words = vec![0u64; (indices.len() * bits).div_ceil(64)];
        for (index, value) in indices.iter().enumerate() {
            for bit in 0..bits {
                if value & (1 << bit) != 0 {
                    let position = index * bits + bit;
                    words[position / 64] |= 1 << (position % 64);
                }
            }
        }
        LongArray::new(words.into_iter().map(|word| word as i64).collect())
    }

    fn region(palette_size: usize, indices: &[usize]) -> Region {
        let bits = (usize::BITS - (palette_size - 1).leading_zeros()).max(2) as usize;
        Region {
            size: Size {
                x: indices.len() as i32,
                y: 1,
                z: 1,
            },
            palette: (0..palette_size)
                .map(|index| state(&format!("minecraft:test_{index}")))
                .collect(),
            block_states: packed(indices, bits),
            block_entities: Vec::new(),
            entities: Vec::new(),
        }
    }

    fn schematic(region: Region) -> Schematic {
        Schematic {
            version: 6,
            regions: BTreeMap::from([("test region".into(), region)]),
        }
    }

    fn gzipped<T: serde::Serialize>(value: &T) -> Vec<u8> {
        let data = fastnbt::to_bytes(value).unwrap();
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&data).unwrap();
        gzip.finish().unwrap()
    }

    fn compound(fields: &[(&str, Value)]) -> Value {
        Value::Compound(
            fields
                .iter()
                .map(|(name, value)| ((*name).into(), value.clone()))
                .collect(),
        )
    }

    fn replace_palette(document: &mut Value, palette: Vec<Value>) {
        let Value::Compound(root) = document else {
            unreachable!();
        };
        let Value::Compound(regions) = root.get_mut("Regions").unwrap() else {
            unreachable!();
        };
        let Value::Compound(region) = regions.get_mut("test region").unwrap() else {
            unreachable!();
        };
        region.insert("BlockStatePalette".into(), Value::List(palette));
    }

    #[test]
    fn modern_block_state_field_names_preserve_properties() {
        let expected = BlockState {
            name: "minecraft:poplar_log".into(),
            properties: BTreeMap::from([("axis".into(), "z".into())]),
        };
        for (name_key, properties_key) in [
            ("id", "properties"),
            ("Name", "properties"),
            ("id", "Properties"),
        ] {
            let mut input = fastnbt::to_value(schematic(region(1, &[0; 4]))).unwrap();
            replace_palette(
                &mut input,
                vec![compound(&[
                    (name_key, Value::String(expected.name.clone())),
                    (
                        properties_key,
                        compound(&[("axis", Value::String("z".into()))]),
                    ),
                ])],
            );
            let actual = read_gzip_data(gzipped(&input).as_slice()).unwrap();
            assert_eq!(actual.states, BTreeMap::from([(expected.clone(), 4)]));
        }
    }

    #[test]
    fn compact_default_block_state_palette_decodes_across_longs() {
        let indices: Vec<_> = (0..40).map(|index| index % 5).collect();
        let mut input = fastnbt::to_value(schematic(region(5, &indices))).unwrap();
        let names = [
            "minecraft:air",
            "minecraft:stone",
            "minecraft:poplar_planks",
            "minecraft:red_wool_slab",
            "minecraft:hopper",
        ];
        replace_palette(
            &mut input,
            names
                .into_iter()
                .map(|name| Value::String(name.into()))
                .collect(),
        );
        let actual = read_gzip_data(gzipped(&input).as_slice()).unwrap();
        assert_eq!(
            actual.states,
            names.into_iter().map(|name| (state(name), 8)).collect()
        );
    }

    #[test]
    fn malformed_modern_block_states_keep_field_and_palette_context() {
        for (entry, expected) in [
            (compound(&[("id", Value::Int(1))]), "id"),
            (
                compound(&[
                    ("id", Value::String("minecraft:stone".into())),
                    ("properties", Value::String("invalid".into())),
                ]),
                "properties",
            ),
            (
                compound(&[
                    ("id", Value::String("minecraft:stone".into())),
                    ("Name", Value::String("minecraft:dirt".into())),
                ]),
                "duplicate field",
            ),
        ] {
            let mut input = fastnbt::to_value(schematic(region(1, &[0]))).unwrap();
            replace_palette(&mut input, vec![entry]);
            let error = read_gzip_data(gzipped(&input).as_slice()).unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains("test region"));
            assert!(message.contains("palette entry 0"));
            assert!(message.contains(expected), "{message}");
        }
    }

    #[test]
    fn inventories_and_entities_are_retained_across_regions() {
        let item = compound(&[
            ("id", Value::String("minecraft:diamond".into())),
            ("Count", Value::Byte(32)),
        ]);
        let chest = compound(&[
            ("id", Value::String("minecraft:chest".into())),
            ("Items", Value::List(vec![item.clone()])),
        ]);
        let hopper = compound(&[
            ("id", Value::String("minecraft:hopper".into())),
            ("Items", Value::List(vec![item.clone()])),
        ]);
        let frame = compound(&[
            ("id", Value::String("minecraft:item_frame".into())),
            ("Item", item),
        ]);
        let mut first = region(2, &[0, 1]);
        first.block_entities.push(chest.clone());
        let mut second = region(2, &[1]);
        second.block_entities.push(hopper.clone());
        second.entities.push(frame.clone());
        let document = Schematic {
            version: 6,
            regions: BTreeMap::from([("first".into(), first), ("second".into(), second)]),
        };
        let actual = read_gzip_data(gzipped(&document).as_slice()).unwrap();
        assert_eq!(actual.states.values().sum::<u64>(), 3);
        assert_eq!(actual.block_entities, [chest, hopper]);
        assert_eq!(actual.entities, [frame]);
    }

    #[test]
    fn version_one_unwraps_tile_nbt_and_entity_data() {
        let chest = compound(&[("id", Value::String("minecraft:chest".into()))]);
        let frame = compound(&[("id", Value::String("minecraft:item_frame".into()))]);
        let mut input = region(1, &[0]);
        input.block_entities = vec![compound(&[("TileNBT", chest.clone())])];
        input.entities = vec![compound(&[("EntityData", frame.clone())])];
        let mut document = schematic(input);
        document.version = 1;
        let actual = read_gzip_data(gzipped(&document).as_slice()).unwrap();
        assert_eq!(actual.block_entities, [chest]);
        assert_eq!(actual.entities, [frame]);
    }

    #[test]
    fn missing_optional_payload_lists_are_empty() {
        let mut input = fastnbt::to_value(schematic(region(1, &[0]))).unwrap();
        let Value::Compound(root) = &mut input else {
            unreachable!();
        };
        let Value::Compound(regions) = root.get_mut("Regions").unwrap() else {
            unreachable!();
        };
        let Value::Compound(region) = regions.get_mut("test region").unwrap() else {
            unreachable!();
        };
        region.remove("TileEntities");
        region.remove("Entities");
        let actual = read_gzip_data(gzipped(&input).as_slice()).unwrap();
        assert!(actual.block_entities.is_empty());
        assert!(actual.entities.is_empty());
        assert_eq!(actual.states.values().sum::<u64>(), 1);
    }

    #[test]
    fn noncompound_payload_entries_keep_region_and_field_context() {
        for field in ["TileEntities", "Entities"] {
            let mut input = region(1, &[0]);
            let payload = vec![Value::Int(12)];
            if field == "TileEntities" {
                input.block_entities = payload;
            } else {
                input.entities = payload;
            }
            let error = read_gzip_data(gzipped(&schematic(input)).as_slice()).unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains("test region"));
            assert!(message.contains(&format!("{field} entry 0 must be a compound")));
        }
    }

    #[test]
    fn malformed_version_one_wrappers_are_rejected() {
        for payload in [
            compound(&[]),
            compound(&[("TileNBT", Value::String("invalid".into()))]),
        ] {
            let mut input = region(1, &[0]);
            input.block_entities.push(payload);
            let mut document = schematic(input);
            document.version = 1;
            let error = read_gzip_data(gzipped(&document).as_slice()).unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains("test region"));
            assert!(message.contains("TileEntities entry 0"));
            assert!(message.contains("TileNBT"));
        }
    }

    #[test]
    fn decodes_three_five_and_six_bit_entries_crossing_longs() {
        for palette_size in [5, 17, 33] {
            let indices: Vec<_> = (0..257)
                .map(|index| (index * 7 + 3) % palette_size)
                .collect();
            let input = region(palette_size, &indices);
            assert!(input.block_states.iter().any(|word| *word < 0));
            let mut expected = StateCounts::new();
            for index in &indices {
                *expected.entry(input.palette[*index].clone()).or_default() += 1;
            }
            let actual = read_gzip(gzipped(&schematic(input)).as_slice()).unwrap();
            assert_eq!(actual, expected, "palette size {palette_size}");
        }
    }

    #[test]
    fn one_entry_palette_still_uses_two_bits_and_counts_air() {
        let mut input = region(1, &[0; 33]);
        input.palette[0] = state("minecraft:air");
        assert_eq!(input.block_states.len(), 2);
        let counts = count_schematic(schematic(input)).unwrap();
        assert_eq!(counts, BTreeMap::from([(state("minecraft:air"), 33)]));
    }

    #[test]
    fn power_of_two_palette_does_not_add_an_extra_bit() {
        let indices: Vec<_> = (0..100).map(|index| index % 16).collect();
        let input = region(16, &indices);
        assert_eq!(input.block_states.len(), 7);
        assert_eq!(
            count_schematic(schematic(input))
                .unwrap()
                .values()
                .sum::<u64>(),
            100
        );
    }

    #[test]
    fn negative_dimensions_count_absolute_volume() {
        let mut input = region(2, &[1; 12]);
        input.size = Size {
            x: -3,
            y: -2,
            z: -2,
        };
        assert_eq!(
            count_schematic(schematic(input))
                .unwrap()
                .values()
                .sum::<u64>(),
            12
        );
    }

    #[test]
    fn duplicate_palette_states_and_regions_are_aggregated() {
        let mut input = region(3, &[0, 1, 2, 1, 2]);
        input.palette[2] = input.palette[1].clone();
        let mut other = region(2, &[1, 1]);
        other.palette[1]
            .properties
            .insert("axis".into(), "x".into());
        let mut document = schematic(input);
        document.regions.insert("other".into(), other);
        let counts = count_schematic(document).unwrap();
        assert_eq!(counts[&state("minecraft:test_1")], 4);
        assert_eq!(counts.len(), 3);
        assert_eq!(counts.values().sum::<u64>(), 7);
    }

    #[test]
    fn padding_bits_do_not_add_blocks() {
        let mut input = region(2, &[1; 3]);
        input.block_states = LongArray::new(vec![-1i64 << 6 | 0b01_01_01]);
        let counts = count_schematic(schematic(input)).unwrap();
        assert_eq!(counts[&state("minecraft:test_1")], 3);
    }

    #[test]
    fn truncated_arrays_are_rejected_before_counting() {
        let mut input = region(2, &[1; 33]);
        input.block_states = LongArray::new(vec![0]);
        let error = count_schematic(schematic(input)).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("test region"));
        assert!(message.contains("truncated"));
        assert!(message.contains("require 2 longs, found 1"));
    }

    #[test]
    fn invalid_indexes_report_block_index_and_coordinates() {
        let mut input = region(5, &[0; 24]);
        let mut indices = vec![0; 24];
        indices[21] = 7;
        input.size = Size { x: 3, y: 2, z: 4 };
        input.block_states = packed(&indices, 3);
        let error = count_schematic(schematic(input)).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("test region"));
        assert!(message.contains("block 21 (x=0, y=1, z=3)"));
        assert!(message.contains("invalid palette index 7"));
    }

    #[test]
    fn empty_palette_and_names_are_rejected() {
        let mut input = region(1, &[0]);
        input.palette.clear();
        let error = count_schematic(schematic(input)).unwrap_err();
        assert!(format!("{error:#}").contains("BlockStatePalette is empty"));
        let mut input = region(1, &[0]);
        input.palette[0].name = " ".into();
        let error = count_schematic(schematic(input)).unwrap_err();
        assert!(format!("{error:#}").contains("palette entry 0 has an empty Name"));
    }

    #[test]
    fn zero_and_overflowing_dimensions_are_rejected() {
        for size in [
            Size { x: 0, y: 1, z: 1 },
            Size {
                x: i32::MIN,
                y: 1,
                z: 1,
            },
            Size {
                x: i32::MAX,
                y: i32::MAX,
                z: i32::MAX,
            },
        ] {
            let mut input = region(1, &[0]);
            input.size = size;
            assert!(count_schematic(schematic(input)).is_err());
        }
    }

    #[test]
    fn accepts_known_versions_and_rejects_unknown_versions() {
        for version in 1..=7 {
            let mut input = schematic(region(1, &[0]));
            input.version = version;
            assert!(count_schematic(input).is_ok());
        }
        for version in [-1, 0, 8] {
            let mut input = schematic(region(1, &[0]));
            input.version = version;
            assert!(count_schematic(input).is_err());
        }
    }

    #[test]
    fn malformed_palette_deserialization_keeps_region_and_entry_context() {
        let mut input = fastnbt::to_value(schematic(region(1, &[0]))).unwrap();
        let Value::Compound(root) = &mut input else {
            panic!("fixture root should be a compound");
        };
        let Value::Compound(regions) = root.get_mut("Regions").unwrap() else {
            panic!("fixture Regions should be a compound");
        };
        let Value::Compound(region) = regions.get_mut("test region").unwrap() else {
            panic!("fixture region should be a compound");
        };
        region.insert(
            "BlockStatePalette".into(),
            Value::List(vec![Value::Compound(Default::default())]),
        );
        let error = read_gzip(gzipped(&input).as_slice()).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("test region"));
        assert!(message.contains("palette entry 0"));
        assert!(message.contains("Name"));
    }

    #[test]
    fn unknown_metadata_fields_are_ignored() {
        let mut input = fastnbt::to_value(schematic(region(1, &[0]))).unwrap();
        if let Value::Compound(root) = &mut input {
            root.insert("Metadata".into(), Value::Compound(Default::default()));
            root.insert("MinecraftDataVersion".into(), Value::Int(999999));
        }
        assert_eq!(
            read_gzip(gzipped(&input).as_slice())
                .unwrap()
                .values()
                .sum::<u64>(),
            1
        );
    }

    #[test]
    fn invalid_gzip_and_truncated_nbt_are_rejected() {
        assert!(read_gzip(b"not gzip".as_slice()).is_err());
        let document = schematic(region(1, &[0]));
        let input = gzipped(&document);
        assert!(read_gzip(&input[..input.len() - 5]).is_err());
        let mut corrupted = input;
        let crc_index = corrupted.len() - 8;
        corrupted[crc_index] ^= 1;
        assert!(read_gzip(corrupted.as_slice()).is_err());

        let mut truncated_nbt = fastnbt::to_bytes(&document).unwrap();
        truncated_nbt.pop();
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&truncated_nbt).unwrap();
        assert!(read_gzip(gzip.finish().unwrap().as_slice()).is_err());
    }

    #[test]
    fn combined_count_overflow_is_rejected() {
        let input = region(1, &[0]);
        let mut counts = BTreeMap::from([(input.palette[0].clone(), u64::MAX)]);
        assert!(count_region(input, &mut counts).is_err());
    }
}
