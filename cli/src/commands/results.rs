use std::path::Path;

use comfy_table::Table;
use comfy_table::presets::UTF8_FULL_CONDENSED;
use owo_colors::OwoColorize;

use crate::style;

pub fn list(root: &Path) -> anyhow::Result<()> {
    println!("{}", style::section("Results"));

    let results_dir = root.join("results");
    if !results_dir.exists() {
        println!("  No results directory found.");
        println!("  Run {} to generate results.", style::info("experiment smoke"));
        return Ok(());
    }

    let mut entries: Vec<_> = std::fs::read_dir(&results_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "csv" || ext == "json"))
        .collect();

    entries.sort_by_key(|e| e.file_name());

    if entries.is_empty() {
        println!("  No result files found.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_header(vec!["File", "Size", "Modified"]);

    for entry in &entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let meta = entry.metadata()?;
        let size = format_size(meta.len());
        let modified = format_time(meta.modified().ok());

        table.add_row(vec![name, size, modified]);
    }

    println!("{table}");
    println!("\n  {} files in results/", entries.len());
    Ok(())
}

pub fn show(
    root: &Path,
    file: &str,
    limit: usize,
    columns: Option<&[String]>,
    filter: Option<&str>,
) -> anyhow::Result<()> {
    let path = resolve_result_path(root, file)?;
    let display_name = path.file_name().unwrap_or_default().to_string_lossy();

    println!("{}", style::section(&format!("Results: {display_name}")));

    if path.extension().is_some_and(|e| e == "json") {
        let content = std::fs::read_to_string(&path)?;
        let value: serde_json::Value = serde_json::from_str(&content)?;
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    // Parse filter (key=value)
    let filter_pair: Option<(String, String)> = filter.and_then(|f| {
        let parts: Vec<&str> = f.splitn(2, '=').collect();
        if parts.len() == 2 {
            Some((parts[0].to_string(), parts[1].to_string()))
        } else {
            style::print_warning(&format!("Invalid filter format: '{f}'. Use key=value."));
            None
        }
    });

    // CSV
    let mut rdr = csv::Reader::from_path(&path)?;
    let all_headers: Vec<String> = rdr.headers()?.iter().map(|h| h.to_string()).collect();

    // Determine which column indices to show
    let col_indices: Vec<usize> = if let Some(cols) = columns {
        cols.iter().filter_map(|c| all_headers.iter().position(|h| h == c)).collect()
    } else {
        (0..all_headers.len()).collect()
    };

    if col_indices.is_empty() {
        anyhow::bail!("No matching columns found. Available: {}", all_headers.join(", "));
    }

    // Find filter column index
    let filter_col =
        filter_pair.as_ref().and_then(|(key, _)| all_headers.iter().position(|h| h == key));

    if filter_pair.is_some() && filter_col.is_none() {
        let key = &filter_pair.as_ref().unwrap().0;
        style::print_warning(&format!(
            "Filter column '{key}' not found. Available: {}",
            all_headers.join(", ")
        ));
    }

    let display_headers: Vec<&str> = col_indices.iter().map(|&i| all_headers[i].as_str()).collect();

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_header(&display_headers);

    let mut shown = 0;
    let mut total = 0;

    for result in rdr.records() {
        let record = result?;
        total += 1;

        // Apply filter
        if let (Some(col), Some((_, val))) = (filter_col, &filter_pair) {
            if let Some(cell) = record.get(col) {
                if cell != val.as_str() {
                    continue;
                }
            }
        }

        if limit > 0 && shown >= limit {
            continue; // keep counting total
        }

        let row: Vec<String> =
            col_indices.iter().map(|&i| record.get(i).unwrap_or("").to_string()).collect();
        table.add_row(row);
        shown += 1;
    }

    println!("{table}");

    let mut footer = format!("  {shown} rows shown");
    if limit > 0 && shown < total {
        footer.push_str(&format!(" (of {total} total, use {} for more)", style::info("--limit 0")));
    }
    if let Some((ref key, ref val)) = filter_pair {
        footer.push_str(&format!(" [filter: {key}={val}]"));
    }
    println!("{footer}");

    Ok(())
}

pub fn summary(root: &Path) -> anyhow::Result<()> {
    println!("{}", style::section("Results Summary"));

    let results_dir = root.join("results");
    if !results_dir.exists() {
        println!("  No results directory found.");
        return Ok(());
    }

    let pattern = results_dir.join("*_summary.csv").to_string_lossy().to_string();

    let files: Vec<_> = glob::glob(&pattern)?.filter_map(|p| p.ok()).collect();

    if files.is_empty() {
        println!("  No summary CSV files found (searched for *_summary.csv).");
        return Ok(());
    }

    for path in &files {
        let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();

        println!("\n{}", style::section(&name));

        let mut rdr = csv::Reader::from_path(path)?;
        let headers: Vec<String> = rdr.headers()?.iter().map(|h| h.to_string()).collect();

        let mut table = Table::new();
        table.load_preset(UTF8_FULL_CONDENSED);
        table.set_header(&headers);

        for result in rdr.records() {
            let record = result?;
            let row: Vec<String> = record.iter().map(|f| f.to_string()).collect();
            table.add_row(row);
        }

        println!("{table}");
    }

    Ok(())
}

pub fn compare(root: &Path, a: &str, b: &str) -> anyhow::Result<()> {
    let path_a = resolve_result_path(root, a)?;
    let path_b = resolve_result_path(root, b)?;

    let name_a = path_a.file_name().unwrap_or_default().to_string_lossy();
    let name_b = path_b.file_name().unwrap_or_default().to_string_lossy();

    println!("{}", style::section(&format!("Compare: {name_a} vs {name_b}")));

    let records_a = read_csv_records(&path_a)?;
    let records_b = read_csv_records(&path_b)?;

    if records_a.headers != records_b.headers {
        style::print_warning("Files have different column headers.");
    }

    let headers = &records_a.headers;
    let max_rows = records_a.rows.len().max(records_b.rows.len());

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);

    let mut header = vec!["Row".to_string()];
    for h in headers {
        header.push(format!("{h} (A)"));
        header.push(format!("{h} (B)"));
        header.push(format!("{h} \u{0394}"));
    }
    table.set_header(header);

    for i in 0..max_rows.min(20) {
        let mut row = vec![format!("{}", i + 1)];
        for col in 0..headers.len() {
            let val_a = records_a.rows.get(i).and_then(|r| r.get(col)).map_or("", |s| s.as_str());
            let val_b = records_b.rows.get(i).and_then(|r| r.get(col)).map_or("", |s| s.as_str());

            row.push(val_a.to_string());
            row.push(val_b.to_string());

            let delta = match (val_a.parse::<f64>(), val_b.parse::<f64>()) {
                (Ok(a_num), Ok(b_num)) => {
                    let d = b_num - a_num;
                    if d.abs() < 0.001 {
                        style::dim("=")
                    } else if d > 0.0 {
                        format!(
                            "{}",
                            format!("+{d:.3}").truecolor(
                                style::SUCCESS.0,
                                style::SUCCESS.1,
                                style::SUCCESS.2
                            )
                        )
                    } else {
                        format!(
                            "{}",
                            format!("{d:.3}").truecolor(
                                style::ERROR.0,
                                style::ERROR.1,
                                style::ERROR.2
                            )
                        )
                    }
                }
                _ => {
                    if val_a == val_b {
                        style::dim("=")
                    } else {
                        style::warning("~")
                    }
                }
            };
            row.push(delta);
        }
        table.add_row(row);
    }

    println!("{table}");

    if max_rows > 20 {
        println!("\n  {} (showing first 20 of {})", style::dim("truncated"), max_rows);
    }

    Ok(())
}

pub fn clean(root: &Path) -> anyhow::Result<()> {
    let results_dir = root.join("results");
    if !results_dir.exists() {
        println!("  No results directory to clean.");
        return Ok(());
    }

    // Count files before asking
    let file_count = std::fs::read_dir(&results_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .count();

    if file_count == 0 {
        println!("  No files in results/ to remove.");
        return Ok(());
    }

    let confirm = dialoguer::Confirm::new()
        .with_prompt(format!("Remove {file_count} files from results/?"))
        .default(false)
        .interact()?;

    if !confirm {
        println!("  Cancelled.");
        return Ok(());
    }

    let mut removed = 0;
    for entry in std::fs::read_dir(&results_dir)? {
        let entry = entry?;
        if entry.path().is_file() {
            std::fs::remove_file(entry.path())?;
            removed += 1;
        }
    }

    style::print_success(&format!("Removed {removed} files from results/"));
    Ok(())
}

pub fn open(root: &Path) -> anyhow::Result<()> {
    let results_dir = root.join("results");
    if !results_dir.exists() {
        std::fs::create_dir_all(&results_dir)?;
    }
    open::that(&results_dir)?;
    style::print_success("Opened results/ in file manager.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn resolve_result_path(root: &Path, file: &str) -> anyhow::Result<std::path::PathBuf> {
    let results_dir = root.join("results");

    let exact = results_dir.join(file);
    if exact.exists() {
        return Ok(exact);
    }

    let with_csv = results_dir.join(format!("{file}.csv"));
    if with_csv.exists() {
        return Ok(with_csv);
    }

    let summary = results_dir.join(format!("{file}_summary.csv"));
    if summary.exists() {
        return Ok(summary);
    }

    let runs = results_dir.join(format!("{file}_runs.csv"));
    if runs.exists() {
        return Ok(runs);
    }

    let json = results_dir.join(format!("{file}.json"));
    if json.exists() {
        return Ok(json);
    }

    anyhow::bail!("File not found: '{}'. Run 'results list' to see available files.", file);
}

struct CsvData {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

fn read_csv_records(path: &Path) -> anyhow::Result<CsvData> {
    let mut rdr = csv::Reader::from_path(path)?;
    let headers: Vec<String> = rdr.headers()?.iter().map(|h| h.to_string()).collect();
    let mut rows = Vec::new();
    for result in rdr.records() {
        let record = result?;
        rows.push(record.iter().map(|f| f.to_string()).collect());
    }
    Ok(CsvData { headers, rows })
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn format_time(time: Option<std::time::SystemTime>) -> String {
    match time {
        Some(t) => {
            let duration = std::time::SystemTime::now().duration_since(t).unwrap_or_default();
            let secs = duration.as_secs();
            if secs < 60 {
                format!("{secs}s ago")
            } else if secs < 3600 {
                format!("{}m ago", secs / 60)
            } else if secs < 86400 {
                format!("{}h ago", secs / 3600)
            } else {
                format!("{}d ago", secs / 86400)
            }
        }
        None => "unknown".to_string(),
    }
}
