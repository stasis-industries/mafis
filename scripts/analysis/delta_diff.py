#!/usr/bin/env python3
"""Delta-diff of pre-fix vs post-fix aisle-width sweep.

Quantifies the kick-back-bug bias per cell so the paper can disclose
the drift in a single paragraph. For each (solver, topology, scenario,
scheduler, num_agents) key we compute Δ = post - pre on the headline
metrics and report both absolute and percentage shifts.

Run AFTER both sweeps have written their summary CSVs.

Output: `results/aisle_width/post_kickback_fix/delta_diff.csv` + stdout
summary of the largest shifts.
"""

from pathlib import Path

import pandas as pd

SRC = Path("results/aisle_width")
POST = SRC / "post_kickback_fix"

TOPO_RENAME = {
    "warehouse_sd_w2": "warehouse_single_dock_w2",
    "warehouse_sd_w3": "warehouse_single_dock_w3",
}

# Metrics to compare (each has _mean suffix in summary CSVs where applicable).
HEADLINE_METRICS = [
    "ft_mean",
    "throughput_mean",
    "critical_time_mean",
    "cascade_spread_mean",
    "structural_cascade_mean",
    "mitigation_delta_mean",
]

KEY_COLS = ["solver", "topology", "scenario", "scheduler", "num_agents"]


def load(path: Path) -> pd.DataFrame:
    df = pd.read_csv(path)
    if "topology" in df.columns:
        df["topology"] = df["topology"].replace(TOPO_RENAME)
    return df


def collect(prefix_dir: Path, matrix_names: list[str]) -> pd.DataFrame:
    frames = []
    for name in matrix_names:
        p = prefix_dir / f"{name}_summary.csv"
        if p.exists():
            frames.append(load(p))
    if not frames:
        return pd.DataFrame()
    return pd.concat(frames, ignore_index=True)


def main() -> None:
    pre_names = [
        "aisle_width_w1",
        "aisle_width_w2_in_env",
        "aisle_width_w2_out_env",
        "aisle_width_w3_in_env",
        "aisle_width_w3_out_env",
    ]
    post_names = [
        "aisle_width_w1_mh",
        "aisle_width_w2_in_env",
        "aisle_width_w2_out_env",
        "aisle_width_w3_in_env",
        "aisle_width_w3_out_env",
    ]

    pre = collect(SRC, pre_names)
    post = collect(POST, post_names)

    if pre.empty or post.empty:
        print(
            f"ERROR: one side missing. pre rows={len(pre)}, post rows={len(post)}. "
            "Re-run sweep(s) first."
        )
        return

    merged = pre.merge(
        post, on=KEY_COLS, suffixes=("_pre", "_post"), how="inner"
    )
    print(f"Matched {len(merged)} cells across both sweeps.")

    records = []
    for _, r in merged.iterrows():
        rec = {k: r[k] for k in KEY_COLS}
        for m in HEADLINE_METRICS:
            a = r.get(f"{m}_pre")
            b = r.get(f"{m}_post")
            if pd.notna(a) and pd.notna(b):
                rec[f"{m}_pre"] = a
                rec[f"{m}_post"] = b
                rec[f"{m}_delta"] = b - a
                rec[f"{m}_pct"] = (100.0 * (b - a) / a) if a != 0 else float("nan")
        records.append(rec)

    delta = pd.DataFrame(records)
    delta.to_csv(POST / "delta_diff.csv", index=False)
    print(f"Wrote {POST}/delta_diff.csv ({len(delta)} rows)")

    for m in HEADLINE_METRICS:
        col = f"{m}_pct"
        if col not in delta.columns:
            continue
        sub = delta.dropna(subset=[col]).copy()
        if sub.empty:
            continue
        sub["abs_pct"] = sub[col].abs()
        print(f"\n=== {m}: largest |Δ%| shifts (top 5) ===")
        show = sub.sort_values("abs_pct", ascending=False).head(5)
        for _, r in show.iterrows():
            print(
                f"  {r['solver']:<15} {r['topology']:<28} "
                f"{r['scenario']:<25} n={int(r['num_agents']):<3} "
                f"pre={r[f'{m}_pre']:.4f} post={r[f'{m}_post']:.4f} "
                f"Δ={r[f'{m}_delta']:+.4f} ({r[col]:+.2f}%)"
            )
        mean_abs_pct = sub[col].abs().mean()
        print(f"  mean |Δ%|: {mean_abs_pct:.2f}%")


if __name__ == "__main__":
    main()
