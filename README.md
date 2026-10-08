# schematicNeeded

A Rust CLI that combines **multiple Minecraft `.litematic` schematics into one CSV or Excel materials list**. It counts placement blocks, saved inventory contents, and entity materials for **vanilla Minecraft Java 26.3**.

- Combine files, directories, and quoted glob patterns in one run.
- Choose `.csv` or `.xlsx` at startup, or select a format through a CLI flag.
- Include saved chest and hopper contents, nested inventories, and copied entities.
- Process schematics in parallel with a configurable worker count.
- Inspect raw block counts or individual block states when needed.

[Quick start](#quick-start) · [Usage](#usage) · [Output](#output) · [Counting behavior](#counting-behavior) · [Development](#development)

## Quick start

Install a current stable [Rust toolchain with Cargo](https://rustup.rs). Clone or download this repository, then open a terminal in its root directory.

Build the executable:

```sh
cargo build --release --locked
```

Combine three schematics into a single report:

```sh
cargo run --release --locked -- house.litematic tower.litematic farm.litematic -o materials
```

At startup, enter `xlsx`, `.xlsx`, or `1` for Excel; enter `csv`, `.csv`, or `2` for CSV. The command creates `materials.xlsx` or `materials.csv` in the current directory. Choices are case insensitive, and blank or invalid input prompts again.

Once built, you can run the executable directly without Cargo:

**Windows (PowerShell)**

```powershell
.\target\release\schematicNeeded.exe house.litematic tower.litematic --format xlsx -o materials.xlsx
```

**Linux / macOS**

```sh
./target/release/schematicNeeded house.litematic tower.litematic --format csv -o materials.csv
```

The executable includes the vanilla registry and needs no network connection or external data files at runtime.

## Usage

```text
schematicNeeded [OPTIONS] <INPUT>...
```

Every selected schematic contributes to **one output file**. Supply one or more file paths, directories, or quoted glob patterns.

### Examples

Export every schematic in a directory to Excel:

```sh
cargo run --release --locked -- schematics --format xlsx -o materials.xlsx
```

Include subdirectories and limit processing to four workers:

```sh
cargo run --release --locked -- schematics --recursive --threads 4 --format csv -o materials.csv
```

Combine matching files from several directories:

```sh
cargo run --release --locked -- "schematics/*.litematic" "other/*.litematic" --format xlsx -o materials.xlsx
```

Inspect stored block states separately:

```sh
cargo run --release --locked -- house.litematic tower.litematic --states --format csv -o states.csv
```

Use `--format csv` or `--format xlsx` to skip the startup prompt in scripts and when input is redirected. Quote glob patterns so the CLI handles them consistently across shells.

### Options

| Option | Description |
| --- | --- |
| `INPUT...` | Files, directories, or quoted glob patterns to combine. |
| `--format csv\|xlsx` | Choose the output format and skip the startup prompt. |
| `-o, --output FILE` | Output path. Defaults to `needed_blocks.csv` or `needed_blocks.xlsx`. |
| `-r, --recursive` | Search subdirectories of directory inputs. |
| `-j, --threads N` | Maximum workers; a positive integer. Defaults to available CPUs, capped by file count. |
| `--raw-blocks` | Count stored block positions without material conversions, inventories, or entities. |
| `--states` | Count raw block states separately, including their sorted properties. |
| `--include-air` | Include normal, cave, and void air; requires `--raw-blocks` or `--states`. |
| `--exclude-contents` | Omit saved container contents, entity equipment, and displayed items. |
| `--exclude-entities` | Omit saved entities and their contents. |
| `-h, --help` | Show CLI help. |
| `-V, --version` | Show the program version. |

Directory scans select `.litematic` files and do not follow directory symlinks. Repeated paths and overlapping file/directory/glob selections are deduplicated using canonical paths. Each distinct file is counted once; different files containing the same build still contribute separately. All regions are summed, including overlapping regions.

## Output

Both formats contain `block` and `count` columns, with quantities summed across all inputs and rows sorted by material ID. The `block` column also holds inventory items and entity materials.

Example CSV:

```csv
block,count
minecraft:oak_planks,120
minecraft:stone,640
```

Excel output is a real `.xlsx` workbook with a `Materials` worksheet. Item metadata remains distinguishable in material labels; CSV quotes labels containing commas, and XLSX stores labels as literal text. Long metadata, such as book text, is preserved in a separate `Details` worksheet. Excel counts above 999,999,999,999,999 are stored as text to preserve their exact value.

The output extension follows the selected format: `--format xlsx -o materials.csv` creates `materials.xlsx`. The parent directory must already exist. Existing output is replaced only after every input has been read and the complete report has been written successfully. Invalid input returns a nonzero exit code and preserves the previous output.

## Counting behavior

The bundled Minecraft Java 26.3 registry contains **1,286 block IDs** and **1,658 item IDs**. Material rules are tested against every block's default state and every allowed property value.

### Blocks

Air is omitted and facing states are combined. Counts reflect placement quantities:

- Double slabs need two slab items; door, bed, and tall plant halves contribute one item per complete block.
- Snow layers, candles, turtle eggs, sea pickles, petals, and leaf litter use their quantity properties.
- Wall variants, planted crops, redstone wire, tripwire, and farmland map to their inventory materials.
- Potted plants contribute both the flower pot and its plant.
- Fluid sources, waterlogged blocks, and submerged plants contribute buckets. Filled cauldrons, charged respawn anchors, and inserted ender eyes contribute their supplies.
- Transient states such as piston heads and portals contribute no separate placement item.

### Saved inventories and entities

Chest and hopper contents are included alongside other vanilla inventories, such as barrels, shulker boxes, furnaces, dispensers, droppers, crafters, brewing stands, chiseled bookshelves, and campfires. Lectern books, jukebox records, and decorated-pot contents are counted. Nested shulker boxes, bundles, and loaded projectiles are traversed. Item metadata distinguishes potions, enchanted items, books, and other variants.

Saved item frames, glow item frames, armor stands, paintings, minecarts, boats, chest boats, rafts, end crystals, and cushions contribute their placement items. Displayed items, equipment, cargo, dropped items, and passengers are included. Copied mobs contribute registered spawn eggs and equipment. Command-only or transient entities without normal placement items are omitted; unsupported entity mappings produce an error.

### Limits

- Contents and entities must be saved by the schematic exporter. Missing data cannot be recovered.
- Unresolved loot tables produce an error. Open the container before exporting, or use `--exclude-contents`.
- Quantities describe finished items and source placements. Crafting ingredients, reusable buckets, infinite water sources, growth, and entity behavior are not calculated.
- The material registry targets vanilla Minecraft Java 26.3. Unknown or modded IDs produce errors in material mode.
- `--raw-blocks` and `--states` provide a block-only census and skip saved inventories and entities.

## Compatibility

The reader accepts gzip-compressed Java NBT `.litematic` format versions **1 through 7**, including multiple regions, signed region dimensions, and packed palette indices spanning 64-bit words. It supports `Name`/`Properties` palettes, 26.3 `id`/`properties` palettes, compact string states, and legacy Version 1 entity wrappers.

The target Minecraft data version is **5023**. The CLI does not run Minecraft's DataFixer. Older schematics whose IDs need conversion beyond the supported rename aliases should be upgraded in Litematica before using material mode.

## Development

Run the project checks from the repository root:

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
```

Tests cover the full block registry and property domains, nested inventories, saved entities, palette formats, packed-word boundaries, invalid data, and CSV/XLSX serialization.

Rayon workers decompress, parse, and count each schematic. Only compact count maps are retained between files; blocks are not expanded into individual objects. Use `--threads 1` to reduce peak memory for large inputs.

### Registry data

The snapshots are stored as JSON:

- [Block IDs](data/vanilla_blocks_26_3.json)
- [Item IDs](data/vanilla_items_26_3.json)
- [Default block states and property domains](data/vanilla_block_states_26_3.json)

[build.rs](build.rs) compiles the block and item registries into static perfect-hash lookup tables. Runtime lookups require no JSON parsing or registry allocation. Block-state JSON supplies exhaustive tests and is not loaded by the CLI.

The data comes from [misode/mcmeta](https://github.com/misode/mcmeta/tree/d96c75fec200c4580dd75e033a76341521165461), pinned to commit `d96c75fec200c4580dd75e033a76341521165461`. The upstream [registry report](https://raw.githubusercontent.com/misode/mcmeta/d96c75fec200c4580dd75e033a76341521165461/registries/data.json), [block-state report](https://raw.githubusercontent.com/misode/mcmeta/d96c75fec200c4580dd75e033a76341521165461/blocks/data.json), and [version metadata](https://raw.githubusercontent.com/misode/mcmeta/d96c75fec200c4580dd75e033a76341521165461/version.json) preserve its provenance.

The decoder and material rules were checked against Litematica's [schematic reader](https://github.com/sakura-ryoko/litematica/blob/26.3/src/main/java/fi/dy/masa/litematica/schematic/LitematicaSchematic.java), [packed block storage](https://github.com/sakura-ryoko/litematica/blob/26.3/src/main/java/fi/dy/masa/litematica/schematic/container/LitematicaBitArray.java), and [material cache](https://github.com/sakura-ryoko/litematica/blob/26.3/src/main/java/fi/dy/masa/litematica/materials/MaterialCache.java).

When reporting a bug, include the command, full error message, Minecraft and exporter versions, and a minimal schematic that reproduces the problem.
