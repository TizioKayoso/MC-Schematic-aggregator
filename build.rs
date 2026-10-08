use std::collections::BTreeSet;
use std::env;
use std::error::Error;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::Path;

fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir = env::var_os("CARGO_MANIFEST_DIR").ok_or("missing CARGO_MANIFEST_DIR")?;
    let output_dir = env::var_os("OUT_DIR").ok_or("missing OUT_DIR")?;
    let mut output = BufWriter::new(File::create(
        Path::new(&output_dir).join("vanilla_registry.rs"),
    )?);

    for (file, name) in [
        ("data/vanilla_blocks_26_3.json", "VANILLA_BLOCKS"),
        ("data/vanilla_items_26_3.json", "VANILLA_ITEMS"),
    ] {
        println!("cargo:rerun-if-changed={file}");
        let path = Path::new(&manifest_dir).join(file);
        let entries: Vec<String> = serde_json::from_reader(BufReader::new(File::open(&path)?))
            .map_err(|error| {
                io::Error::new(io::ErrorKind::InvalidData, format!("{file}: {error}"))
            })?;
        let mut unique = BTreeSet::new();
        let mut registry = phf_codegen::Set::new();

        if entries.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{file}: registry must not be empty"),
            )
            .into());
        }
        for entry in &entries {
            if entry.is_empty()
                || !entry.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'_' | b'/' | b'.' | b'-')
                })
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{file}: invalid unnamespaced ID {entry:?}"),
                )
                .into());
            }
            if !unique.insert(entry.as_str()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{file}: duplicate ID {entry:?}"),
                )
                .into());
            }
            registry.entry(entry.as_str());
        }
        writeln!(
            output,
            "static {name}: phf::Set<&'static str> = {};",
            registry.build()
        )?;
    }
    output.flush()?;
    Ok(())
}
