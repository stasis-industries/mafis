use std::path::Path;

use comfy_table::presets::UTF8_FULL_CONDENSED;
use comfy_table::Table;
use owo_colors::OwoColorize;

use crate::style;

struct Constant {
    name: String,
    value: String,
    doc: String,
    section: String,
    cfg: Option<String>,
}

fn parse_constants(root: &Path) -> anyhow::Result<Vec<Constant>> {
    let path = root.join("src/constants.rs");
    let content = std::fs::read_to_string(&path)?;

    let mut constants = Vec::new();
    let mut current_section = "General".to_string();
    let mut current_doc = String::new();
    let mut current_cfg: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();

        // Section header: // ── Name ──...
        if trimmed.starts_with("// \u{2500}\u{2500}") || trimmed.starts_with("// --") {
            // Extract section name
            let name = trimmed
                .trim_start_matches("// ")
                .trim_start_matches('\u{2500}')
                .trim_start_matches('-')
                .trim_start()
                .trim_end_matches('\u{2500}')
                .trim_end_matches('-')
                .trim();
            if !name.is_empty() {
                current_section = name.to_string();
            }
            current_doc.clear();
            current_cfg = None;
            continue;
        }

        // Doc comment
        if trimmed.starts_with("///") {
            let doc = trimmed.trim_start_matches("///").trim();
            if !current_doc.is_empty() {
                current_doc.push(' ');
            }
            current_doc.push_str(doc);
            continue;
        }

        // cfg attribute
        if trimmed.starts_with("#[cfg(") {
            let cfg = trimmed
                .trim_start_matches("#[cfg(")
                .trim_end_matches(")]")
                .to_string();
            current_cfg = Some(cfg);
            continue;
        }

        // Constant definition
        if trimmed.starts_with("pub const ") {
            if let Some((name_type, value)) = trimmed
                .trim_start_matches("pub const ")
                .split_once('=')
            {
                let name = name_type
                    .split(':')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();

                let value = value
                    .trim()
                    .trim_end_matches(';')
                    .trim()
                    .to_string();

                constants.push(Constant {
                    name,
                    value,
                    doc: std::mem::take(&mut current_doc),
                    section: current_section.clone(),
                    cfg: current_cfg.take(),
                });
            }
            continue;
        }

        // Regular comment or blank line resets doc
        if trimmed.is_empty() || (trimmed.starts_with("//") && !trimmed.starts_with("///")) {
            if !trimmed.starts_with("//!") {
                current_doc.clear();
                current_cfg = None;
            }
        }
    }

    Ok(constants)
}

pub fn show(root: &Path) -> anyhow::Result<()> {
    println!("{}", style::section("Configuration Constants"));

    let constants = parse_constants(root)?;
    if constants.is_empty() {
        println!("  No constants found in src/constants.rs");
        return Ok(());
    }

    let mut current_section = String::new();

    for c in &constants {
        if c.section != current_section {
            current_section = c.section.clone();
            println!("\n{}", style::section(&current_section));

            let mut table = Table::new();
            table.load_preset(UTF8_FULL_CONDENSED);
            table.set_header(vec!["Constant", "Value", "Description"]);

            // Add all constants in this section
            for inner in constants.iter().filter(|ic| ic.section == current_section) {
                let name_display = if let Some(ref cfg) = inner.cfg {
                    format!("{} [{}]", inner.name, cfg)
                } else {
                    inner.name.clone()
                };

                let doc = if inner.doc.len() > 60 {
                    format!("{}...", &inner.doc[..57])
                } else {
                    inner.doc.clone()
                };

                table.add_row(vec![name_display, inner.value.clone(), doc]);
            }

            println!("{table}");
        }
    }

    println!(
        "\n  Source: {}",
        style::info("src/constants.rs")
    );
    Ok(())
}

pub fn get(root: &Path, key: &str) -> anyhow::Result<()> {
    let constants = parse_constants(root)?;

    let key_upper = key.to_uppercase();
    let found: Vec<&Constant> = constants
        .iter()
        .filter(|c| c.name.to_uppercase().contains(&key_upper))
        .collect();

    if found.is_empty() {
        anyhow::bail!(
            "No constant matching '{}'. Run 'config show' to see all.",
            key
        );
    }

    for c in &found {
        println!("{}", style::section(&c.name));
        style::kv("Value", &c.value.bold().to_string());
        style::kv("Section", &c.section);
        if let Some(ref cfg) = c.cfg {
            style::kv("Platform", cfg);
        }
        if !c.doc.is_empty() {
            style::kv("Description", &c.doc);
        }
        println!();
    }

    Ok(())
}
