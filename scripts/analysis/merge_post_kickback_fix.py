#!/usr/bin/env python3
"""Merge the post-kick-back-fix re-run CSVs with the pre-fix sweep.

For the aisle-width paper dataset, we produce one coherent CSV set where:
  - SD-w1 n=20 cells come from the pre-fix sweep (`results/aisle_width/`) —
    low-pressure, <1% drift from the kick-back fix, not re-run.
  - All other cells (SD-w1 n={40,60}, SD-w2 × {36,72,108}, SD-w3 × {50,100,151})
    come from the post-fix re-run (`results/aisle_width/post_kickback_fix/`).

Also normalizes the topology column: pre-fix rows carry the old ids
(`warehouse_sd_w2`, `warehouse_sd_w3`), post-fix rows carry the new ids
(`warehouse_single_dock_w2`, `warehouse_single_dock_w3`). After merge every
row uses the new id.

Output: `results/aisle_width/merged_post_fix/<matrix>_{runs,summary}.csv`
(aisle_width_w1_runs, aisle_width_w2_in_env_runs, ...).

Run AFTER the post-fix sweep completes.
"""

from pathlib import Path

import pandas as pd

SRC = Path("results/aisle_width")
POST = SRC / "post_kickback_fix"
OUT = SRC / "merged_post_fix"
OUT.mkdir(parents=True, exist_ok=True)

# Topology id remap — applies to any row from the pre-fix sweep.
TOPO_RENAME = {
    "warehouse_sd_w2": "warehouse_single_dock_w2",
    "warehouse_sd_w3": "warehouse_single_dock_w3",
}


def normalize(df: pd.DataFrame) -> pd.DataFrame:
    """Rewrite legacy topology ids to the current scheme."""
    if "topology" in df.columns:
        df["topology"] = df["topology"].replace(TOPO_RENAME)
    return df


def sd_w1_low_density(kind: str) -> pd.DataFrame:
    """Return only SD-w1 n=20 rows from the pre-fix w1 sweep (kept as-is)."""
    df = pd.read_csv(SRC / f"aisle_width_w1_{kind}.csv")
    return normalize(df[df["num_agents"] == 20].copy())


def sd_w1_mh(kind: str) -> pd.DataFrame:
    """Post-fix SD-w1 n={40,60}."""
    df = pd.read_csv(POST / f"aisle_width_w1_mh_{kind}.csv")
    return normalize(df)


def passthrough_post(name: str, kind: str) -> pd.DataFrame:
    df = pd.read_csv(POST / f"{name}_{kind}.csv")
    return normalize(df)


def merge_one(kind: str) -> None:
    # aisle_width_w1 = low (pre-fix) + mh (post-fix)
    w1 = pd.concat(
        [sd_w1_low_density(kind), sd_w1_mh(kind)],
        ignore_index=True,
    )
    cols = pd.read_csv(SRC / f"aisle_width_w1_{kind}.csv").columns.tolist()
    w1 = w1[cols]
    w1.to_csv(OUT / f"aisle_width_w1_{kind}.csv", index=False)
    print(f"  aisle_width_w1_{kind}: {len(w1)} rows (pre-fix n=20 + post-fix n=40/60)")

    # w2 / w3 come entirely from post-fix
    for name in (
        "aisle_width_w2_in_env",
        "aisle_width_w2_out_env",
        "aisle_width_w3_in_env",
        "aisle_width_w3_out_env",
    ):
        df = passthrough_post(name, kind)
        df.to_csv(OUT / f"{name}_{kind}.csv", index=False)
        print(f"  {name}_{kind}: {len(df)} rows (post-fix)")


def main() -> None:
    for kind in ("runs", "summary"):
        print(f"--- {kind} ---")
        merge_one(kind)
    print(f"\nMerged CSVs written to {OUT}")


if __name__ == "__main__":
    main()
