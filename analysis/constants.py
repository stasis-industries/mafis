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
    "intermittent_80s80m15r",
]

# LaTeX-safe labels (for tables)
SCENARIO_LABEL = {
    "burst_20pct":             "Burst 20\\%",
    "burst_50pct":             "Burst 50\\%",
    "wear_medium":             "Wear (med.)",
    "wear_high":               "Wear (high)",
    "zone_50t":                "Zone (50t)",
    "intermittent_80s80m15r":  "Intermittent",
}

# Short labels (for SVG axes)
SCENARIO_LABEL_SHORT = {
    "burst_20pct":             "Burst 20%",
    "burst_50pct":             "Burst 50%",
    "wear_medium":             "Wear Med",
    "wear_high":               "Wear High",
    "zone_50t":                "Zone 50t",
    "intermittent_80s80m15r":  "Intermit.",
}

SCENARIO_CATEGORY = {
    "burst_20pct":             "Permanent",
    "burst_50pct":             "Permanent",
    "wear_medium":             "Permanent",
    "wear_high":               "Permanent",
    "zone_50t":                "Recoverable",
    "intermittent_80s80m15r":  "Recoverable",
}

# Faithful solvers (sourced from public reference implementations).
SOLVER_ORDER = [
    "pibt",
    "rhcr_pbs",
    "token_passing",
    "lacam3_lifelong",
]

SOLVER_LABEL = {
    "pibt":            "PIBT",
    "rhcr_pbs":        "RHCR-PBS",
    "token_passing":   "Token Passing",
    "lacam3_lifelong": "LaCAM3",
}

SOLVER_COLORS = {
    "pibt":            "#e07b39",
    "rhcr_pbs":        "#5cb85c",
    "token_passing":   "#e74c3c",
    "lacam3_lifelong": "#9b59b6",
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
