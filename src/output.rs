use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use rust_xlsxwriter::{Format, FormatAlign, Workbook};
use tempfile::NamedTempFile;

use crate::materials::is_vanilla_item;

const EXCEL_MAX_ROWS: usize = 1_048_576;
const EXCEL_MAX_CELL_UNITS: usize = 32_767;
// Excel stores only 15 significant decimal digits in numeric cells. Larger
// counts are written as text so opening the workbook cannot round them.
const EXCEL_MAX_EXACT_COUNT: u64 = 999_999_999_999_999;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Csv,
    Xlsx,
}

impl OutputFormat {
    pub const fn extension(&self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Xlsx => "xlsx",
        }
    }
}

pub fn write_output(
    output: &Path,
    counts: &BTreeMap<String, u64>,
    format: OutputFormat,
) -> Result<()> {
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = NamedTempFile::new_in(parent)
        .with_context(|| format!("could not create output in '{}'", parent.display()))?;

    match format {
        OutputFormat::Csv => write_csv(temporary.as_file_mut(), counts),
        OutputFormat::Xlsx => write_xlsx(temporary.as_file_mut(), counts),
    }
    .with_context(|| format!("could not write {} report", format.extension()))?;

    temporary
        .as_file_mut()
        .flush()
        .context("could not flush output")?;
    temporary
        .as_file()
        .sync_all()
        .context("could not save output")?;
    temporary
        .persist(output)
        .with_context(|| format!("could not replace output '{}'", output.display()))?;
    Ok(())
}

fn write_csv(file: &mut File, counts: &BTreeMap<String, u64>) -> Result<()> {
    let mut writer = csv::Writer::from_writer(file);
    writer.write_record(["block", "count"])?;
    for (block, count) in counts {
        writer.write_record([block.as_str(), &count.to_string()])?;
    }
    writer.flush().context("could not flush CSV")?;
    Ok(())
}

fn xlsx_last_row(material_types: usize) -> Result<u32> {
    if material_types >= EXCEL_MAX_ROWS {
        bail!(
            "XLSX supports at most {} material types plus the header; use CSV for this report",
            EXCEL_MAX_ROWS - 1
        );
    }
    Ok(material_types as u32)
}

fn write_xlsx(file: &mut File, counts: &BTreeMap<String, u64>) -> Result<()> {
    let last_row = xlsx_last_row(counts.len())?;
    let mut workbook = Workbook::new();
    let header = Format::new().set_bold();
    let count_format = Format::new()
        .set_num_format("0")
        .set_align(FormatAlign::Right);
    let mut details = Vec::new();
    {
        let worksheet = workbook.add_worksheet();
        worksheet.set_name("Materials")?;
        worksheet.set_column_width(0, 60)?;
        worksheet.set_column_width(1, 22)?;
        worksheet.set_freeze_panes(1, 0)?;
        worksheet.write_string_with_format(0, 0, "block", &header)?;
        worksheet.write_string_with_format(0, 1, "count", &header)?;

        for (index, (block, count)) in counts.iter().enumerate() {
            let row = index as u32 + 1;
            if block.encode_utf16().count() > EXCEL_MAX_CELL_UNITS {
                let base = block
                    .split_once('[')
                    .filter(|(base, suffix)| {
                        is_vanilla_item(base)
                            && suffix.strip_suffix(']').is_some_and(|value| !value.is_empty())
                    })
                    .map(|(base, _)| base)
                    .context("XLSX material names cannot exceed 32,767 UTF-16 code units; use CSV for this report")?;
                worksheet.write_string(row, 0, format!("{base}[details={row}]"))?;
                for (chunk_index, chunk) in label_chunks(block).into_iter().enumerate() {
                    if details.len() >= EXCEL_MAX_ROWS - 1 {
                        bail!(
                            "XLSX Details worksheet exceeds the row limit; use CSV for this report"
                        );
                    }
                    details.push((row, chunk_index as u32 + 1, chunk));
                }
            } else {
                worksheet.write_string(row, 0, block)?;
            }
            if *count <= EXCEL_MAX_EXACT_COUNT {
                worksheet.write_number_with_format(row, 1, *count as f64, &count_format)?;
            } else {
                worksheet.write_string_with_format(row, 1, count.to_string(), &count_format)?;
            }
        }
        if !counts.is_empty() {
            worksheet.autofilter(0, 0, last_row, 1)?;
        }
    }
    if !details.is_empty() {
        let worksheet = workbook.add_worksheet();
        worksheet.set_name("Details")?;
        worksheet.set_column_width(0, 16)?;
        worksheet.set_column_width(1, 12)?;
        worksheet.set_column_width(2, 100)?;
        worksheet.set_freeze_panes(1, 0)?;
        worksheet.write_string_with_format(0, 0, "reference", &header)?;
        worksheet.write_string_with_format(0, 1, "chunk", &header)?;
        worksheet.write_string_with_format(0, 2, "value", &header)?;
        for (index, (reference, chunk, value)) in details.iter().enumerate() {
            let row = index as u32 + 1;
            worksheet.write_number(row, 0, f64::from(*reference))?;
            worksheet.write_number(row, 1, f64::from(*chunk))?;
            worksheet.write_string(row, 2, *value)?;
        }
        worksheet.autofilter(0, 0, details.len() as u32, 2)?;
    }
    workbook.save_to_writer(file)?;
    Ok(())
}

fn label_chunks(label: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut units = 0;
    for (index, character) in label.char_indices() {
        if units + character.len_utf16() > EXCEL_MAX_CELL_UNITS {
            chunks.push(&label[start..index]);
            start = index;
            units = 0;
        }
        units += character.len_utf16();
    }
    if start < label.len() {
        chunks.push(&label[start..]);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use std::fs;

    use calamine::{Data, Reader, Xlsx, open_workbook};
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn xlsx_preserves_literal_names_and_exact_counts() {
        let directory = tempdir().unwrap();
        let output = directory.path().join("materials.xlsx");
        let counts = BTreeMap::from([
            ("=literal name".to_owned(), 42),
            ("minecraft:stone".to_owned(), EXCEL_MAX_EXACT_COUNT),
            ("mod:large_count".to_owned(), EXCEL_MAX_EXACT_COUNT + 1),
            ("mod:maximum_count".to_owned(), u64::MAX),
        ]);
        write_output(&output, &counts, OutputFormat::Xlsx).unwrap();

        let mut workbook: Xlsx<_> = open_workbook(&output).unwrap();
        assert_eq!(workbook.sheet_names(), &["Materials"]);
        let range = workbook.worksheet_range("Materials").unwrap();
        let rows: Vec<_> = range.rows().collect();
        assert_eq!(
            rows[0],
            &[Data::String("block".into()), Data::String("count".into())]
        );
        assert_eq!(
            rows[1],
            &[Data::String("=literal name".into()), Data::Float(42.0)]
        );
        assert_eq!(rows[2][1], Data::Float(EXCEL_MAX_EXACT_COUNT as f64));
        assert_eq!(
            rows[3][1],
            Data::String((EXCEL_MAX_EXACT_COUNT + 1).to_string())
        );
        assert_eq!(rows[4][1], Data::String(u64::MAX.to_string()));
        assert!(workbook.worksheet_formula("Materials").unwrap().is_empty());
    }

    #[test]
    fn xlsx_preserves_long_unicode_book_metadata_in_details() {
        let directory = tempdir().unwrap();
        let output = directory.path().join("materials.xlsx");
        let name = format!("minecraft:written_book[pages={}]", "📚".repeat(24_000));
        let counts = BTreeMap::from([("minecraft:stone".into(), 4), (name.clone(), 2)]);
        write_output(&output, &counts, OutputFormat::Xlsx).unwrap();

        let mut workbook: Xlsx<_> = open_workbook(&output).unwrap();
        assert_eq!(workbook.sheet_names(), &["Materials", "Details"]);
        let materials = workbook.worksheet_range("Materials").unwrap();
        assert_eq!(
            materials.get((2, 0)),
            Some(&Data::String("minecraft:written_book[details=2]".into()))
        );
        assert_eq!(materials.get((2, 1)), Some(&Data::Float(2.0)));
        let details = workbook.worksheet_range("Details").unwrap();
        let mut restored = String::new();
        for (index, row) in details.rows().skip(1).enumerate() {
            assert_eq!(row[0], Data::Float(2.0));
            assert_eq!(row[1], Data::Float((index + 1) as f64));
            let Data::String(value) = &row[2] else {
                panic!("detail chunk must contain literal text");
            };
            assert!(value.encode_utf16().count() <= 32_767);
            restored.push_str(value);
        }
        assert_eq!(restored, name);
        assert_eq!(details.height(), 3);

        let csv_output = directory.path().join("materials.csv");
        write_output(&csv_output, &counts, OutputFormat::Csv).unwrap();
        let mut reader = csv::Reader::from_path(csv_output).unwrap();
        let records: Vec<_> = reader.records().map(Result::unwrap).collect();
        assert_eq!(records[1].get(0), Some(name.as_str()));
    }

    #[test]
    fn csv_preserves_quoted_names_and_full_u64_counts() {
        let directory = tempdir().unwrap();
        let output = directory.path().join("materials.csv");
        let name = "mod:block[property=one,other=\"two\"]";
        let counts = BTreeMap::from([(name.to_owned(), u64::MAX)]);
        write_output(&output, &counts, OutputFormat::Csv).unwrap();

        let mut reader = csv::Reader::from_path(output).unwrap();
        assert_eq!(reader.headers().unwrap(), &["block", "count"][..]);
        let rows: Vec<_> = reader.records().map(Result::unwrap).collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get(0), Some(name));
        assert_eq!(rows[0].get(1), Some(u64::MAX.to_string().as_str()));
    }

    #[test]
    fn failed_xlsx_write_preserves_existing_output_and_removes_temporary_file() {
        let directory = tempdir().unwrap();
        let output = directory.path().join("materials.xlsx");
        fs::write(&output, b"existing report").unwrap();
        let counts = BTreeMap::from([("x".repeat(32_768), 1)]);

        let error = write_output(&output, &counts, OutputFormat::Xlsx).unwrap_err();
        assert!(format!("{error:#}").contains("32,767"));
        assert_eq!(fs::read(output).unwrap(), b"existing report");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn empty_xlsx_has_readable_headers() {
        let directory = tempdir().unwrap();
        let output = directory.path().join("materials.xlsx");
        write_output(&output, &BTreeMap::new(), OutputFormat::Xlsx).unwrap();

        let mut workbook: Xlsx<_> = open_workbook(output).unwrap();
        let range = workbook.worksheet_range("Materials").unwrap();
        assert_eq!(range.get_size(), (1, 2));
        assert_eq!(range.get((0, 0)), Some(&Data::String("block".into())));
        assert_eq!(range.get((0, 1)), Some(&Data::String("count".into())));
    }

    #[test]
    fn xlsx_row_limit_reserves_space_for_header() {
        assert_eq!(xlsx_last_row(EXCEL_MAX_ROWS - 1).unwrap(), 1_048_575);
        assert!(xlsx_last_row(EXCEL_MAX_ROWS).is_err());
    }
}
