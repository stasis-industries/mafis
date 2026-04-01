"""
constants.py — Shared display constants for MAFIS experiment analysis scripts.

Imported by all analysis scripts in this directory.
"""

SCENARIO_ORDER = [
    "burst_20pct",
    "burst_50pct",
    "wear_medium",
    "wear_high",
    "zone_50t",
    "intermittent_80m15r",
    "perm_zone_100pct",
]

# LaTeX-safe labels (for tables)
SCENARIO_LABEL = {
    "burst_20pct":         "Burst 20\\%",
    "burst_50pct":         "Burst 50\\%",
    "wear_medium":         "Wear (med.)",
    "wear_high":           "Wear (high)",
    "zone_50t":            "Zone (50t)",
    "intermittent_80m15r": "Intermittent",
    "perm_zone_100pct":    "Perm. Zone",
}

# Short labels (for SVG axes)
SCENARIO_LABEL_SHORT = {
    "burst_20pct":         "Burst 20%",
    "burst_50pct":         "Burst 50%",
    "wear_medium":         "Wear Med",
    "wear_high":           "Wear High",
    "zone_50t":            "Zone 50t",
    "intermittent_80m15r": "Intermit.",
    "perm_zone_100pct":    "Perm Zone",
}

SCENARIO_CATEGORY = {
    "burst_20pct":         "Permanent-distributed",
    "burst_50pct":         "Permanent-distributed",
    "wear_medium":         "Permanent-distributed",
    "wear_high":           "Permanent-distributed",
    "zone_50t":            "Recoverable",
    "intermittent_80m15r": "Recoverable",
    "perm_zone_100pct":    "Permanent-localized",
}

# All 7 lifelong solvers. solver_resilience uses 6 (no rhcr_pbs).
SOLVER_ORDER = [
    "pibt",
    "rhcr_pibt",
    "rhcr_pbs",
    "rhcr_priority_astar",
    "token_passing",
    "rt_lacam",
    "tpts",
]

SOLVER_LABEL = {
    "pibt":                "PIBT",
    "rhcr_pibt":           "RHCR-PIBT",
    "rhcr_pbs":            "RHCR-PBS",
    "rhcr_priority_astar": "RHCR-A*",
    "token_passing":       "Token Passing",
    "rt_lacam":            "RT-LaCAM",
    "tpts":                "TPTS",
}

SOLVER_COLORS = {
    "pibt":                "#e07b39",
    "rhcr_pibt":           "#4a9eca",
    "rhcr_pbs":            "#5cb85c",
    "rhcr_priority_astar": "#9b59b6",
    "token_passing":       "#e74c3c",
    "rt_lacam":            "#f39c12",
    "tpts":                "#1abc9c",
}

DENSITY_ORDER = [10, 20, 40, 80]

SCHEDULER_ORDER = ["random", "closest"]

SCHEDULER_LABEL = {
    "random":  "Random",
    "closest": "Closest",
}

SCHEDULER_COLORS = {
    "random":  "#4a9eca",
    "closest": "#e07b39",
}
