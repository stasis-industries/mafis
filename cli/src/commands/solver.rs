use comfy_table::presets::UTF8_FULL_CONDENSED;
use comfy_table::Table;

use crate::style;

struct SolverInfo {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    details: &'static str,
    reference: &'static str,
}

const SOLVERS: &[SolverInfo] = &[
    SolverInfo {
        id: "pibt",
        name: "PIBT",
        description: "Reactive, one-step priority inheritance; replans every tick",
        details: "Priority Inheritance with Backtracking. Each agent inherits priority \
                  from the highest-priority agent it would collide with. Replans every \
                  tick (zero-cost when no collision). Lifelong-native.",
        reference: "Okumura et al., 2022",
    },
    SolverInfo {
        id: "rhcr_pbs",
        name: "RHCR (PBS)",
        description: "Windowed PBS with node limit; replans every W ticks",
        details: "Rolling-Horizon Collision Resolution using Priority-Based Search. \
                  Plans H steps ahead, replans every W ticks. PBS uses a priority tree \
                  to resolve collisions with a configurable node limit.",
        reference: "Li et al., 2021",
    },
    SolverInfo {
        id: "rhcr_pibt",
        name: "RHCR (PIBT-Window)",
        description: "Unrolled PIBT for H steps; fast, cooperative",
        details: "Rolling-Horizon with PIBT as the inner planner. Unrolls PIBT for H \
                  steps to produce cooperative plans. Fastest RHCR variant. Good \
                  balance of speed and quality.",
        reference: "Li et al., 2021 + Okumura et al., 2022",
    },
    SolverInfo {
        id: "rhcr_priority_astar",
        name: "RHCR (Priority A*)",
        description: "Sequential spacetime A*; good for moderate density",
        details: "Rolling-Horizon with sequential Priority A* planning. Each agent \
                  plans a path using spacetime A* while treating higher-priority \
                  agents' plans as dynamic obstacles.",
        reference: "Li et al., 2021",
    },
    SolverInfo {
        id: "token_passing",
        name: "Token Passing",
        description: "Decentralized sequential planning via shared TOKEN; \u{2264}100 agents",
        details: "Decentralized sequential planning. A shared TOKEN stores all agents' \
                  planned paths. Idle agents plan via spacetime A* against constraints \
                  built from TOKEN paths. PIBT_MAPD-style prioritization: tasked \
                  agents plan first.",
        reference: "Ma et al., 2017",
    },
];

pub fn list() -> anyhow::Result<()> {
    println!("{}", style::section("Solvers"));
    println!();

    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_header(vec!["ID", "Name", "Description"]);

    for s in SOLVERS {
        table.add_row(vec![
            s.id.to_string(),
            s.name.to_string(),
            s.description.to_string(),
        ]);
    }

    println!("{table}");
    println!();
    println!(
        "  Details: {}",
        style::info("solver info <id>")
    );
    Ok(())
}

pub fn info(name: &str) -> anyhow::Result<()> {
    let solver = SOLVERS
        .iter()
        .find(|s| s.id == name || s.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Unknown solver '{}'. Run 'solver list' to see options.",
                name
            )
        })?;

    println!("{}", style::section(solver.name));
    println!();
    style::kv("ID", solver.id);
    style::kv("Description", solver.description);
    style::kv("Reference", solver.reference);
    println!();
    println!("  {}", solver.details);

    Ok(())
}
