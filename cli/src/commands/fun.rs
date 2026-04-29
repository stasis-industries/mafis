use std::io::Write;
use std::path::Path;

use owo_colors::OwoColorize;

use crate::style;

// ---------------------------------------------------------------------------
// Simple PRNG (avoids pulling in rand for visual effects)
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        Self(seed ^ 0xdeadbeef)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn range(&mut self, max: usize) -> usize {
        (self.next() % max as u64) as usize
    }
}

// ---------------------------------------------------------------------------
// Clear
// ---------------------------------------------------------------------------

pub fn clear() -> anyhow::Result<()> {
    use crossterm::{execute, terminal::{Clear, ClearType}, cursor::MoveTo};
    let mut stdout = std::io::stdout();
    execute!(stdout, Clear(ClearType::All), MoveTo(0, 0))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Matrix Rain
// ---------------------------------------------------------------------------

pub fn rain() -> anyhow::Result<()> {
    use crossterm::{
        cursor::{Hide, Show},
        execute,
        style::ResetColor,
        terminal::{
            disable_raw_mode, enable_raw_mode, size, Clear, ClearType,
            EnterAlternateScreen, LeaveAlternateScreen,
        },
    };

    let mut stdout = std::io::stdout();
    let (term_cols, term_rows) = size()?;
    let cols = term_cols as usize;
    let rows = term_rows as usize;

    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, Hide, Clear(ClearType::All))?;

    // Ensure cleanup on any exit
    let result = run_rain_loop(&mut stdout, cols, rows);

    execute!(stdout, Show, ResetColor, LeaveAlternateScreen)?;
    disable_raw_mode()?;

    result
}

fn run_rain_loop(
    stdout: &mut impl Write,
    cols: usize,
    rows: usize,
) -> anyhow::Result<()> {
    use crossterm::{
        cursor::MoveTo,
        event::{poll, read, Event},
        execute,
        style::{Color, Print, SetForegroundColor},
    };
    use std::time::Duration;

    let rain_chars: Vec<char> =
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789@#$%&*+=<>{}[]|/\\~`^"
            .chars()
            .collect();

    let mapf_words: &[&str] = &[
        "MAPF", "PIBT", "RHCR", "FAULT", "AGENT", "GRID", "PATH", "TASK",
        "MAFIS", "BURST", "WEAR", "ZONE", "TICK", "PLAN", "GOAL",
    ];

    let mut rng = Rng::new();

    // Per-column state
    struct Drop {
        y: f64,
        speed: f64,
        tail_len: usize,
        active: bool,
    }

    let mut drops: Vec<Drop> = (0..cols)
        .map(|_| Drop {
            y: -(rng.range(rows * 2) as f64),
            speed: 0.3 + (rng.range(20) as f64) * 0.1,
            tail_len: 4 + rng.range(16),
            active: rng.range(3) == 0,
        })
        .collect();

    // Grid state
    let mut brightness: Vec<Vec<u8>> = vec![vec![0; cols]; rows];
    let mut chars: Vec<Vec<char>> = vec![vec![' '; cols]; rows];
    let mut frame = 0u64;

    loop {
        if poll(Duration::from_millis(35))? {
            if let Event::Key(_) = read()? {
                break;
            }
        }

        // Decay brightness
        for row in &mut brightness {
            for cell in row.iter_mut() {
                *cell = cell.saturating_sub(6);
            }
        }

        // Update drops
        for x in 0..cols {
            let drop = &mut drops[x];

            if !drop.active {
                if rng.range(40) == 0 {
                    drop.active = true;
                    drop.y = 0.0;
                    drop.speed = 0.3 + (rng.range(20) as f64) * 0.1;
                    drop.tail_len = 4 + rng.range(16);
                }
                continue;
            }

            drop.y += drop.speed;
            let yi = drop.y as usize;

            if yi < rows {
                chars[yi][x] = rain_chars[rng.range(rain_chars.len())];
                brightness[yi][x] = 255;
            }

            // Randomly flicker trail characters
            if yi > 1 && rng.range(8) == 0 {
                let flicker_y = yi.saturating_sub(1 + rng.range(drop.tail_len.min(yi)));
                if flicker_y < rows {
                    chars[flicker_y][x] = rain_chars[rng.range(rain_chars.len())];
                }
            }

            if yi > rows + drop.tail_len + 10 {
                drop.active = false;
            }
        }

        // Occasionally flash a MAPF word
        if frame % 20 == 0 && rng.range(3) == 0 {
            let word = mapf_words[rng.range(mapf_words.len())];
            let wx = rng.range(cols.saturating_sub(word.len()));
            let wy = rng.range(rows);
            for (i, ch) in word.chars().enumerate() {
                if wx + i < cols {
                    chars[wy][wx + i] = ch;
                    brightness[wy][wx + i] = 255;
                }
            }
        }

        // Render
        for y in 0..rows {
            for x in 0..cols {
                let b = brightness[y][x];
                if b > 0 {
                    let color = if b > 220 {
                        // Head: bright white-green
                        Color::Rgb {
                            r: 180,
                            g: 255,
                            b: 180,
                        }
                    } else if b > 150 {
                        // Body: brand amber
                        Color::Rgb {
                            r: (b as u16 * 200 / 255) as u8,
                            g: (b as u16 * 170 / 255) as u8,
                            b: (b as u16 * 80 / 255) as u8,
                        }
                    } else if b > 60 {
                        // Mid: green
                        Color::Rgb {
                            r: 0,
                            g: b,
                            b: 0,
                        }
                    } else {
                        // Tail: dark green
                        Color::Rgb {
                            r: 0,
                            g: b / 2,
                            b: 0,
                        }
                    };
                    execute!(
                        stdout,
                        MoveTo(x as u16, y as u16),
                        SetForegroundColor(color),
                        Print(chars[y][x]),
                    )?;
                } else if chars[y][x] != ' ' {
                    chars[y][x] = ' ';
                    execute!(stdout, MoveTo(x as u16, y as u16), Print(' '))?;
                }
            }
        }

        stdout.flush()?;
        frame += 1;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Fortune
// ---------------------------------------------------------------------------

const FORTUNES: &[&str] = &[
    "PIBT: an agent inherits priority from whoever it would collide with. Elegant simplicity.",
    "Braess's Paradox: adding more agents can DECREASE overall throughput. More isn't always better.",
    "RHCR plans H steps ahead but only commits to W steps \u{2014} the rest is speculative.",
    "Token Passing is like a talking stick: only the agent holding the token can plan.",
    "Fault Tolerance (FT) = faulted throughput / baseline throughput. Below 1.0 means degraded.",
    "Critical Time: the fraction of simulation below 50% of baseline throughput. Lower is more resilient.",
    "NRR (Non-Recovery Rate): 1.0 means the system never recovered. 0.0 means full recovery.",
    "Wear-based faults: busy agents accumulate heat and break down first. The hardest workers fail.",
    "Burst faults kill N% of agents instantly \u{2014} a stress test for sudden catastrophic failure.",
    "Zone outages disable an entire region \u{2014} like a power outage in one aisle of the warehouse.",
    "The ADG (Action Dependency Graph) reveals which agents are blocking which. See the cascade.",
    "Cascade depth: how many agents are transitively affected by a single fault. Deeper = more fragile.",
    "MAFIS is a fault resilience observatory, not a solver benchmark. We measure what breaks.",
    "The scheduler (random vs closest) affects resilience MORE than the solver algorithm. Surprising.",
    "Determinism: same seed + same config = identical simulation. Every. Single. Time.",
    "500 ticks at steady state \u{2248} 100 completed tasks \u{2014} enough for statistical significance.",
    "5 seeds per configuration gives usable 95% confidence intervals.",
    "PIBT replans every tick. It's purely reactive \u{2014} no memory, no regret, just the next best move.",
    "RHCR replans every W ticks. Between replans, agents follow their pre-computed paths.",
    "The warehouse has three zone types: pickup (shelves), delivery (staging), and corridor (movement).",
];

pub fn fortune() -> anyhow::Result<()> {
    let mut rng = Rng::new();
    let fortune = FORTUNES[rng.range(FORTUNES.len())];

    let (r, g, b) = style::BRAND;
    let (dr, dg, db) = style::DIM;

    println!();
    println!(
        "  {}",
        "\u{250c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2510}"
            .truecolor(dr, dg, db)
    );

    // Word-wrap at 58 chars
    let mut line = String::new();
    for word in fortune.split_whitespace() {
        if line.len() + word.len() + 1 > 58 {
            println!(
                "  {} {:<58} {}",
                "\u{2502}".truecolor(dr, dg, db),
                line.truecolor(r, g, b),
                "\u{2502}".truecolor(dr, dg, db),
            );
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        println!(
            "  {} {:<58} {}",
            "\u{2502}".truecolor(dr, dg, db),
            line.truecolor(r, g, b),
            "\u{2502}".truecolor(dr, dg, db),
        );
    }

    println!(
        "  {}",
        "\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2518}"
            .truecolor(dr, dg, db)
    );
    println!();

    Ok(())
}

// ---------------------------------------------------------------------------
// Tree
// ---------------------------------------------------------------------------

pub fn tree(root: &Path) -> anyhow::Result<()> {
    println!("{}", style::section("Project Tree"));
    println!();

    let (r, g, b) = style::BRAND;
    let (ir, ig, ib) = style::INFO;
    let (dr, dg, db) = style::DIM;

    let project_name = root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();

    println!(
        "  {}",
        project_name.truecolor(r, g, b).bold()
    );

    let modules: &[(&str, &str, &str)] = &[
        ("src/core/", "core", "Tick loop, agents, grid, state, topology"),
        ("src/solver/", "solver", "PIBT, RHCR, Token Passing, A*"),
        ("src/fault/", "fault", "Heat/wear, fault detection, scenarios"),
        ("src/analysis/", "analysis", "ADG, cascade, metrics, heatmap"),
        ("src/render/", "render", "3D visuals, materials, orbit camera"),
        ("src/ui/", "ui", "Controls, bridge, desktop"),
        ("src/experiment/", "experiment", "Runner, metrics, export, presets"),
        ("src/sim_tests/", "sim_tests", "Integration test harness"),
    ];

    let total_modules = modules.len();

    for (i, (dir, name, desc)) in modules.iter().enumerate() {
        let is_last = i == total_modules - 1;
        let connector = if is_last { "\u{2514}" } else { "\u{251c}" };
        let line = if is_last { " " } else { "\u{2502}" };

        let full_dir = root.join(dir);
        let (files, lines) = count_dir(&full_dir);

        println!(
            "  {}\u{2500}\u{2500} {}{}  {}",
            connector.truecolor(dr, dg, db),
            name.truecolor(ir, ig, ib).bold(),
            format!("/ ({files} files, {lines} lines)").truecolor(dr, dg, db),
            desc.truecolor(dr, dg, db),
        );
        let _ = line; // used for child items if we had them
    }

    // Non-src directories
    println!(
        "  {}\u{2500}\u{2500} {}  {}",
        "\u{251c}".truecolor(dr, dg, db),
        "web/".truecolor(ir, ig, ib).bold(),
        "JS/HTML/CSS shell + charts".truecolor(dr, dg, db),
    );

    let tests_dir = root.join("tests");
    let (tf, tl) = count_dir(&tests_dir);
    println!(
        "  {}\u{2500}\u{2500} {}{}",
        "\u{251c}".truecolor(dr, dg, db),
        "tests/".truecolor(ir, ig, ib).bold(),
        format!(" ({tf} files, {tl} lines)").truecolor(dr, dg, db),
    );

    let results_count = root
        .join("results")
        .read_dir()
        .map(|d| d.filter_map(|e| e.ok()).count())
        .unwrap_or(0);
    println!(
        "  {}\u{2500}\u{2500} {}{}",
        "\u{251c}".truecolor(dr, dg, db),
        "results/".truecolor(ir, ig, ib).bold(),
        format!(" ({results_count} files)").truecolor(dr, dg, db),
    );

    println!(
        "  {}\u{2500}\u{2500} {}  {}",
        "\u{2514}".truecolor(dr, dg, db),
        "cli/".truecolor(r, g, b).bold(),
        "this tool".truecolor(dr, dg, db),
    );

    println!();
    Ok(())
}

fn count_dir(dir: &Path) -> (usize, usize) {
    let pattern = dir.join("**/*.rs").to_string_lossy().to_string();
    let mut files = 0;
    let mut lines = 0;
    for path in glob::glob(&pattern).into_iter().flatten().flatten() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            files += 1;
            lines += content.lines().count();
        }
    }
    (files, lines)
}
