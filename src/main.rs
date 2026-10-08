use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Parser;
use rayon::prelude::*;
use schematic_needed::contents::{add_block_entity, add_entity_with_contents};
use schematic_needed::litematic::{BlockState, read_schematic};
use schematic_needed::materials::required_materials;
use schematic_needed::output::{OutputFormat, write_output};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Combine Minecraft 26.3 schematic blocks, inventory contents, and entity materials into one CSV or XLSX",
    after_help = "Examples:\n  schematicNeeded house.litematic tower.litematic\n  schematicNeeded \"schematics/*.litematic\" --format xlsx -o materials.xlsx\n  schematicNeeded schematics --recursive --threads 4 --format csv\n\nAt startup, choose .xlsx or .csv. Use --format to skip the prompt in scripts.\nEach file is counted once. All regions contribute to the combined total.\nUse --raw-blocks for stored block counts, or --states to inspect full block states."
)]
struct Args {
    #[arg(required = true, num_args = 1.., value_name = "INPUT", help = ".litematic files, directories, or quoted glob patterns to combine")]
    inputs: Vec<PathBuf>,

    #[arg(
        short,
        long,
        value_name = "FILE",
        help = "Output file (default: needed_blocks.<format>; extension follows chosen format)"
    )]
    output: Option<PathBuf>,

    #[arg(
        long,
        value_enum,
        help = "Output format; skip the startup question for scripts"
    )]
    format: Option<OutputFormat>,

    #[arg(
        short,
        long,
        help = "Include subdirectories when an input is a directory"
    )]
    recursive: bool,

    #[arg(short = 'j', long, value_parser = parse_threads, value_name = "N", help = "Maximum worker threads (default: available CPUs, capped by file count)")]
    threads: Option<usize>,

    #[arg(
        long,
        help = "Count block positions only, without material conversions, inventories, or entities"
    )]
    raw_blocks: bool,

    #[arg(
        long,
        help = "Count raw block states separately, including their sorted properties"
    )]
    states: bool,

    #[arg(
        long,
        help = "Include air in raw counts (requires --raw-blocks or --states)"
    )]
    include_air: bool,

    #[arg(
        long,
        help = "Exclude saved container contents, entity equipment, and displayed items"
    )]
    exclude_contents: bool,

    #[arg(long, help = "Exclude all saved entities and their contents")]
    exclude_entities: bool,
}

fn parse_threads(value: &str) -> std::result::Result<usize, String> {
    let count: usize = value
        .parse()
        .map_err(|_| "expected a positive integer".to_owned())?;
    if count == 0 {
        return Err("thread count must be at least 1".to_owned());
    }
    Ok(count)
}

fn main() {
    if let Err(error) = run(Args::parse()) {
        eprintln!("Error: {error:#}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<()> {
    if args.include_air && !(args.raw_blocks || args.states) {
        bail!("--include-air requires --raw-blocks or --states");
    }

    let format = match args.format {
        Some(format) => format,
        None => prompt_output_format()?,
    };
    let output = output_path(args.output.as_deref(), format)?;
    let files = discover_inputs(&args.inputs, args.recursive)?;
    for path in args.output.iter().chain(std::iter::once(&output)) {
        if let Ok(canonical) = path.canonicalize()
            && files.contains(&canonical)
        {
            bail!("output '{}' is also an input schematic", path.display());
        }
    }

    let threads = args
        .threads
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, usize::from))
        .min(files.len());
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .context("could not create worker threads")?;

    let per_file: Vec<BTreeMap<String, u64>> = pool.install(|| {
        files
            .par_iter()
            .map(|path| count_file(path, &args))
            .collect::<Result<Vec<_>>>()
    })?;

    let mut totals = BTreeMap::new();
    for counts in per_file {
        for (block, count) in counts {
            add_count(&mut totals, block, count)?;
        }
    }

    let total = totals.values().try_fold(0_u64, |sum, count| {
        sum.checked_add(*count)
            .context("combined block count exceeds u64")
    })?;
    write_output(&output, &totals, format)?;
    eprintln!(
        "Combined {} schematic(s): {} block/material type(s), {} total. Wrote {}",
        files.len(),
        totals.len(),
        total,
        output.display()
    );
    Ok(())
}

fn prompt_output_format() -> Result<OutputFormat> {
    let mut input = io::stdin().lock();
    let mut prompt = io::stderr().lock();
    writeln!(
        prompt,
        "Choose output format:\n  1) .xlsx (Excel workbook)\n  2) .csv"
    )?;
    let mut choice = String::new();
    loop {
        write!(prompt, "Enter .xlsx or .csv (1/2): ")?;
        prompt
            .flush()
            .context("could not display output format question")?;
        choice.clear();
        if input
            .read_line(&mut choice)
            .context("could not read output format choice")?
            == 0
        {
            bail!(
                "no output format provided; use --format csv or --format xlsx when running without interactive input"
            );
        }
        match choice.trim().to_ascii_lowercase().as_str() {
            "xlsx" | ".xlsx" | "1" => return Ok(OutputFormat::Xlsx),
            "csv" | ".csv" | "2" => return Ok(OutputFormat::Csv),
            _ => writeln!(prompt, "Please enter .xlsx or .csv (1/2).")?,
        }
    }
}

fn output_path(requested: Option<&Path>, format: OutputFormat) -> Result<PathBuf> {
    let mut output = requested.map_or_else(
        || PathBuf::from(format!("needed_blocks.{}", format.extension())),
        Path::to_path_buf,
    );
    if output.file_name().is_none() || output.is_dir() {
        bail!("output '{}' must name a file", output.display());
    }
    if !output
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(format.extension()))
    {
        output.set_extension(format.extension());
    }
    Ok(output)
}

fn count_file(path: &Path, args: &Args) -> Result<BTreeMap<String, u64>> {
    let data = read_schematic(path)?;
    let mut counts = BTreeMap::new();
    for (state, count) in data.states {
        if args.raw_blocks || args.states {
            if !args.include_air && is_air(&state.name) {
                continue;
            }
            let key = if args.states {
                state_label(&state)
            } else {
                state.name
            };
            add_count(&mut counts, key, count)?;
        } else {
            let materials = required_materials(&state).with_context(|| {
                format!(
                    "invalid block state '{}' in '{}'",
                    state_label(&state),
                    path.display()
                )
            })?;
            for (material, multiplier) in materials {
                let amount = count
                    .checked_mul(multiplier)
                    .context("material count exceeds u64")?;
                add_count(&mut counts, material, amount)?;
            }
        }
    }
    if !(args.raw_blocks || args.states) {
        if !args.exclude_contents {
            for (index, block_entity) in data.block_entities.iter().enumerate() {
                add_block_entity(&mut counts, block_entity)
                    .with_context(|| format!("block entity {index} in '{}'", path.display()))?;
            }
        }
        if !args.exclude_entities {
            for (index, entity) in data.entities.iter().enumerate() {
                add_entity_with_contents(&mut counts, entity, !args.exclude_contents)
                    .with_context(|| format!("entity {index} in '{}'", path.display()))?;
            }
        }
    }
    Ok(counts)
}

fn is_air(name: &str) -> bool {
    matches!(
        name,
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

fn state_label(state: &BlockState) -> String {
    if state.properties.is_empty() {
        return state.name.clone();
    }
    let properties = state
        .properties
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("{}[{properties}]", state.name)
}

fn add_count(counts: &mut BTreeMap<String, u64>, block: String, amount: u64) -> Result<()> {
    if amount == 0 {
        return Ok(());
    }
    let count = counts.entry(block.clone()).or_default();
    *count = count
        .checked_add(amount)
        .with_context(|| format!("count for '{block}' exceeds u64"))?;
    Ok(())
}

fn discover_inputs(inputs: &[PathBuf], recursive: bool) -> Result<Vec<PathBuf>> {
    let mut files = BTreeSet::new();
    let mut visited_directories = BTreeSet::new();
    for input in inputs {
        let has_pattern = input
            .to_str()
            .is_some_and(|path| path.contains(['*', '?', '[']));
        match input.try_exists() {
            Ok(true) => {
                add_input(input, recursive, &mut files, &mut visited_directories)?;
                continue;
            }
            Ok(false) => {}
            // Windows rejects wildcard filenames before glob expansion.
            Err(error)
                if has_pattern
                    && (error.kind() == std::io::ErrorKind::InvalidInput
                        || (cfg!(windows) && error.raw_os_error() == Some(123))) => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("could not inspect '{}'", input.display()));
            }
        }

        let pattern = input
            .to_str()
            .context("glob pattern must be valid Unicode")?;
        if !has_pattern {
            bail!("input '{}' does not exist", input.display());
        }
        let mut matches = 0;
        let options = glob::MatchOptions {
            case_sensitive: !cfg!(windows),
            ..Default::default()
        };
        for entry in glob::glob_with(pattern, options)
            .with_context(|| format!("invalid glob pattern '{pattern}'"))?
        {
            let path = entry.with_context(|| format!("could not expand glob '{pattern}'"))?;
            add_input(&path, recursive, &mut files, &mut visited_directories)?;
            matches += 1;
        }
        if matches == 0 {
            bail!("glob pattern '{pattern}' matched no inputs");
        }
    }
    if files.is_empty() {
        bail!("no .litematic files found in the supplied inputs");
    }
    Ok(files.into_iter().collect())
}

fn add_input(
    path: &Path,
    recursive: bool,
    files: &mut BTreeSet<PathBuf>,
    visited_directories: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    let metadata =
        fs::metadata(path).with_context(|| format!("could not inspect '{}'", path.display()))?;
    if metadata.is_dir() {
        let canonical = path
            .canonicalize()
            .with_context(|| format!("could not resolve '{}'", path.display()))?;
        if !visited_directories.insert(canonical) {
            return Ok(());
        }
        for entry in fs::read_dir(path)
            .with_context(|| format!("could not read directory '{}'", path.display()))?
        {
            let entry =
                entry.with_context(|| format!("could not read entry in '{}'", path.display()))?;
            let entry_path = entry.path();
            let kind = entry
                .file_type()
                .with_context(|| format!("could not inspect '{}'", entry_path.display()))?;
            if kind.is_dir() {
                if recursive {
                    add_input(&entry_path, recursive, files, visited_directories)?;
                }
            } else if is_litematic(&entry_path) {
                let target = fs::metadata(&entry_path)
                    .with_context(|| format!("could not inspect '{}'", entry_path.display()))?;
                if target.is_file() {
                    insert_file(&entry_path, files)?;
                }
            }
        }
    } else if metadata.is_file() {
        if !is_litematic(path) {
            bail!("input '{}' is not a .litematic file", path.display());
        }
        insert_file(path, files)?;
    } else {
        bail!(
            "input '{}' is not a regular file or directory",
            path.display()
        );
    }
    Ok(())
}

fn is_litematic(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("litematic"))
}

fn insert_file(path: &Path, files: &mut BTreeSet<PathBuf>) -> Result<()> {
    files.insert(
        path.canonicalize()
            .with_context(|| format!("could not resolve '{}'", path.display()))?,
    );
    Ok(())
}
