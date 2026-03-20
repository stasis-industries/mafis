use owo_colors::OwoColorize;

// ── Color palette ──────────────────────────────────────────────────────
pub const BRAND: (u8, u8, u8) = (200, 170, 100); // amber/gold
pub const SUCCESS: (u8, u8, u8) = (100, 200, 100); // green
pub const ERROR: (u8, u8, u8) = (200, 80, 80); // red
pub const WARNING: (u8, u8, u8) = (200, 200, 80); // yellow
pub const INFO: (u8, u8, u8) = (100, 200, 220); // cyan
pub const DIM: (u8, u8, u8) = (120, 120, 120); // gray
pub const MUTED: (u8, u8, u8) = (60, 60, 60); // dark gray

pub fn brand(s: &str) -> String {
    format!("{}", s.truecolor(BRAND.0, BRAND.1, BRAND.2))
}

pub fn success(s: &str) -> String {
    format!("{}", s.truecolor(SUCCESS.0, SUCCESS.1, SUCCESS.2))
}

pub fn error(s: &str) -> String {
    format!("{}", s.truecolor(ERROR.0, ERROR.1, ERROR.2))
}

pub fn warning(s: &str) -> String {
    format!("{}", s.truecolor(WARNING.0, WARNING.1, WARNING.2))
}

pub fn info(s: &str) -> String {
    format!("{}", s.truecolor(INFO.0, INFO.1, INFO.2))
}

pub fn dim(s: &str) -> String {
    format!("{}", s.truecolor(DIM.0, DIM.1, DIM.2))
}

pub fn section(title: &str) -> String {
    let pad = 60usize.saturating_sub(title.len() + 4);
    let line = "\u{2500}".repeat(pad);
    format!(
        "{} {} {}",
        "\u{2500}\u{2500}".truecolor(BRAND.0, BRAND.1, BRAND.2),
        title.truecolor(BRAND.0, BRAND.1, BRAND.2).bold(),
        line.truecolor(BRAND.0, BRAND.1, BRAND.2)
    )
}

pub fn print_error(msg: &str) {
    eprintln!(
        "{} {}",
        "\u{2717}".truecolor(ERROR.0, ERROR.1, ERROR.2),
        msg
    );
}

pub fn print_success(msg: &str) {
    println!(
        "{} {}",
        "\u{2713}".truecolor(SUCCESS.0, SUCCESS.1, SUCCESS.2),
        msg,
    );
}

pub fn print_warning(msg: &str) {
    println!(
        "{} {}",
        "\u{26a0}".truecolor(WARNING.0, WARNING.1, WARNING.2),
        msg
    );
}

pub fn kv(key: &str, val: &str) {
    println!(
        "  {:<24} {}",
        key.truecolor(DIM.0, DIM.1, DIM.2),
        val
    );
}
