use std::io::Write;

use owo_colors::OwoColorize;

use crate::style;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn print_logo() {
    let lines = [
        r"    __  ______    _______________",
        r"   /  |/  /   |  / ____/  _/ ___/",
        r"  / /|_/ / /| | / /_   / / \_ \ ",
        r" / /  / / __  |/ __/ _/ / __/ / ",
        r"/_/  /_/_/  |_/_/   /___/____/  ",
    ];
    let (r, g, b) = style::BRAND;
    let (dr, dg, db) = style::DIM;
    println!();
    for line in &lines {
        println!("{}", line.truecolor(r, g, b));
    }
    println!();
    println!(
        "{}  {}",
        "Multi-Agent Fault Injection Simulator".truecolor(r, g, b),
        format!("v{VERSION}").truecolor(dr, dg, db),
    );
    println!();
}

/// Animated typewriter logo for REPL startup.
pub fn print_logo_animated() {
    let term = console::Term::stdout();
    if !term.is_term() {
        print_logo();
        return;
    }

    let lines = [
        r"    __  ______    _______________",
        r"   /  |/  /   |  / ____/  _/ ___/",
        r"  / /|_/ / /| | / /_   / / \_ \ ",
        r" / /  / / __  |/ __/ _/ / __/ / ",
        r"/_/  /_/_/  |_/_/   /___/____/  ",
    ];

    let (r, g, b) = style::BRAND;
    let (dr, dg, db) = style::DIM;

    println!();

    // Typewriter: each character appears with a tiny delay
    let mut stdout = std::io::stdout();
    for line in &lines {
        for ch in line.chars() {
            print!("{}", ch.truecolor(r, g, b));
            let _ = stdout.flush();
            std::thread::sleep(std::time::Duration::from_millis(3));
        }
        println!();
    }

    // Brief pause, then tagline fades in
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Tagline with sweep effect
    let tagline = "Multi-Agent Fault Injection Simulator";
    for (i, ch) in tagline.chars().enumerate() {
        // Gradient from dim to bright
        let progress = i as f64 / tagline.len() as f64;
        let cr = (120.0 + progress * 80.0) as u8;
        let cg = (100.0 + progress * 70.0) as u8;
        let cb = (40.0 + progress * 60.0) as u8;
        print!("{}", ch.truecolor(cr, cg, cb));
        let _ = stdout.flush();
        std::thread::sleep(std::time::Duration::from_millis(8));
    }

    print!("  {}", format!("v{VERSION}").truecolor(dr, dg, db));
    println!();
    println!();
}
