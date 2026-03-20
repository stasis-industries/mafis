use std::path::Path;

use comfy_table::presets::UTF8_FULL_CONDENSED;
use comfy_table::Table;
use owo_colors::OwoColorize;
use serde::Deserialize;

use crate::style;

// ---------------------------------------------------------------------------
// Dynamic topology scanning from web/topologies/*.json
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct TopoJson {
    width: usize,
    height: usize,
    #[serde(default)]
    #[allow(dead_code)]
    seed: u64,
    cells: Vec<CellJson>,
    #[serde(default)]
    robots: Vec<RobotJson>,
}

#[derive(Deserialize)]
struct CellJson {
    x: usize,
    y: usize,
    #[serde(rename = "type")]
    cell_type: String,
}

#[derive(Deserialize)]
struct RobotJson {
    x: usize,
    y: usize,
}

struct ScannedTopology {
    id: String,
    file: String,
    width: usize,
    height: usize,
    walls: usize,
    pickups: usize,
    deliveries: usize,
    robots: usize,
}

impl ScannedTopology {
    fn name(&self) -> String {
        let capitalized: String = self
            .id
            .chars()
            .enumerate()
            .map(|(i, c)| if i == 0 { c.to_ascii_uppercase() } else { c })
            .collect();
        format!("Warehouse {capitalized}")
    }

    #[allow(dead_code)]
    fn description(&self) -> String {
        format!(
            "{}x{}, {} walls, {} pickups, {} deliveries",
            self.width, self.height, self.walls, self.pickups, self.deliveries,
        )
    }
}

fn scan_topologies(root: &Path) -> Vec<ScannedTopology> {
    let dir = root.join("web/topologies");
    if !dir.exists() {
        return vec![];
    }

    let pattern = dir.join("*.json").to_string_lossy().to_string();
    let mut topos: Vec<ScannedTopology> = glob::glob(&pattern)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|p| p.ok())
        .filter_map(|path| {
            let content = std::fs::read_to_string(&path).ok()?;
            let data: TopoJson = serde_json::from_str(&content).ok()?;
            let id = path.file_stem()?.to_string_lossy().to_string();
            let file = path.file_name()?.to_string_lossy().to_string();
            Some(ScannedTopology {
                id,
                file,
                width: data.width,
                height: data.height,
                walls: data.cells.iter().filter(|c| c.cell_type == "wall").count(),
                pickups: data.cells.iter().filter(|c| c.cell_type == "pickup").count(),
                deliveries: data
                    .cells
                    .iter()
                    .filter(|c| c.cell_type == "delivery")
                    .count(),
                robots: data.robots.len(),
            })
        })
        .collect();

    // Sort by grid area (smallest first)
    topos.sort_by_key(|t| t.width * t.height);
    topos
}

pub fn list(root: &Path) -> anyhow::Result<()> {
    println!("{}", style::section("Topologies"));
    println!();

    let topos = scan_topologies(root);

    if topos.is_empty() {
        println!("  No topology files found in web/topologies/");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_header(vec!["ID", "Size", "Robots", "Walls", "Pickups", "Deliveries"]);

    for t in &topos {
        table.add_row(vec![
            t.id.clone(),
            format!("{}x{}", t.width, t.height),
            t.robots.to_string(),
            t.walls.to_string(),
            t.pickups.to_string(),
            t.deliveries.to_string(),
        ]);
    }

    println!("{table}");
    println!();
    println!("  Preview: {}", style::info("topology preview <id>"));
    Ok(())
}

pub fn info(root: &Path, name: &str) -> anyhow::Result<()> {
    let topos = scan_topologies(root);
    let topo = topos
        .iter()
        .find(|t| t.id == name)
        .ok_or_else(|| {
            let available: Vec<&str> = topos.iter().map(|t| t.id.as_str()).collect();
            anyhow::anyhow!(
                "Unknown topology '{}'. Available: {}",
                name,
                available.join(", ")
            )
        })?;

    println!("{}", style::section(&topo.name()));
    println!();
    style::kv("ID", &topo.id);
    style::kv("File", &format!("web/topologies/{}", topo.file));
    style::kv("Grid", &format!("{}x{}", topo.width, topo.height));
    style::kv("Robots", &topo.robots.to_string());
    style::kv("Walls", &topo.walls.to_string());
    style::kv("Pickups", &topo.pickups.to_string());
    style::kv("Deliveries", &topo.deliveries.to_string());
    println!();
    println!(
        "  Preview: {}",
        style::info(&format!("topology preview {}", topo.id))
    );

    Ok(())
}

pub fn preview(root: &Path, name: &str) -> anyhow::Result<()> {
    let topos = scan_topologies(root);
    let topo = topos.iter().find(|t| t.id == name);

    // Try to load directly even if not in scan list
    let json_path = root.join("web/topologies").join(format!("{name}.json"));
    if !json_path.exists() {
        let available: Vec<&str> = topos.iter().map(|t| t.id.as_str()).collect();
        anyhow::bail!(
            "Topology file not found: web/topologies/{name}.json. Available: {}",
            available.join(", ")
        );
    }

    let content = std::fs::read_to_string(&json_path)?;
    let data: TopoJson = serde_json::from_str(&content)?;

    let title = if let Some(t) = topo {
        format!("{} ({}x{})", t.name(), data.width, data.height)
    } else {
        format!("{name} ({}x{})", data.width, data.height)
    };

    println!("{}", style::section(&title));
    println!();

    // Build grid
    let mut grid = vec![vec!['.'; data.width]; data.height];

    for cell in &data.cells {
        if cell.x < data.width && cell.y < data.height {
            let ch = match cell.cell_type.as_str() {
                "wall" => '#',
                "pickup" => 'P',
                "delivery" => 'D',
                _ => '?',
            };
            grid[cell.y][cell.x] = ch;
        }
    }

    for robot in &data.robots {
        if robot.x < data.width && robot.y < data.height {
            grid[robot.y][robot.x] = 'R';
        }
    }

    // Print grid
    for row in &grid {
        print!("  ");
        for &ch in row {
            match ch {
                '#' => print!(
                    "{}",
                    ch.truecolor(style::DIM.0, style::DIM.1, style::DIM.2)
                ),
                'P' => print!(
                    "{}",
                    ch.truecolor(style::BRAND.0, style::BRAND.1, style::BRAND.2)
                ),
                'D' => print!(
                    "{}",
                    ch.truecolor(style::SUCCESS.0, style::SUCCESS.1, style::SUCCESS.2)
                ),
                'R' => print!(
                    "{}",
                    ch.truecolor(style::INFO.0, style::INFO.1, style::INFO.2)
                ),
                _ => print!(
                    "{}",
                    ch.truecolor(style::MUTED.0, style::MUTED.1, style::MUTED.2)
                ),
            }
        }
        println!();
    }

    println!();
    println!(
        "  {} wall  {} pickup  {} delivery  {} robot  {} open",
        style::dim("#"),
        style::brand("P"),
        style::success("D"),
        style::info("R"),
        style::dim("."),
    );
    println!();

    let walls = data.cells.iter().filter(|c| c.cell_type == "wall").count();
    let pickups = data
        .cells
        .iter()
        .filter(|c| c.cell_type == "pickup")
        .count();
    let deliveries = data
        .cells
        .iter()
        .filter(|c| c.cell_type == "delivery")
        .count();

    style::kv("Walls", &walls.to_string());
    style::kv("Pickups", &pickups.to_string());
    style::kv("Deliveries", &deliveries.to_string());
    style::kv("Robots", &data.robots.len().to_string());

    Ok(())
}

pub fn mapmaker(root: &Path) -> anyhow::Result<()> {
    let mapmaker_path = root.join("web/mapmaker.html");
    if !mapmaker_path.exists() {
        anyhow::bail!("mapmaker.html not found in web/");
    }

    open::that(&mapmaker_path)?;
    style::print_success("Opened mapmaker.html in browser.");
    Ok(())
}
