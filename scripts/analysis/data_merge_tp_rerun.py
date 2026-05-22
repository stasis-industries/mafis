#!/usr/bin/env python3
"""Merge the 2026-04-20 TP rerun CSVs into the original aisle-width sweep.

Replaces TP rows in the three in-envelope summary+runs CSVs with the
post-fix data from `results/aisle_width/tp_rerun/`. PIBT + RHCR-PBS rows
and the *_out_env matrices (no TP) pass through unchanged.

Output: `results/aisle_width/merged/<matrix>_{runs,summary}.csv`
"""

from pathlib import Path

import pandas as pd

SRC = Path("results/aisle_width")
TP = SRC / "tp_rerun"
OUT = SRC / "merged"
OUT.mkdir(parents=True, exist_ok=True)

REPLACEMENTS = {
    "aisle_width_w1":         "aisle_width_w1_tp",
    "aisle_width_w2_in_env":  "aisle_width_w2_in_env_tp",
    "aisle_width_w3_in_env":  "aisle_width_w3_in_env_tp",
}
PASSTHROUGH = ["aisle_width_w2_out_env", "aisle_width_w3_out_env"]


def merge(kind: str):
    """kind ∈ {'runs', 'summary'}"""
    for original_name, tp_name in REPLACEMENTS.items():
        original = pd.read_csv(SRC / f"{original_name}_{kind}.csv")
        tp_new = pd.read_csv(TP / f"{tp_name}_{kind}.csv")
        non_tp = original[original["solver"] != "token_passing"]
        merged = pd.concat([non_tp, tp_new], ignore_index=True)
        # Preserve original column order exactly (concat keeps tp_new's if drifted).
        merged = merged[original.columns]
        merged.to_csv(OUT / f"{original_name}_{kind}.csv", index=False)
        print(
            f"  {original_name}_{kind}: "
            f"non-TP={len(non_tp)} + TP_new={len(tp_new)} = {len(merged)} rows"
        )

    for name in PASSTHROUGH:
        df = pd.read_csv(SRC / f"{name}_{kind}.csv")
        df.to_csv(OUT / f"{name}_{kind}.csv", index=False)
        print(f"  {name}_{kind}: passthrough ({len(df)} rows)")


def main():
    for kind in ("runs", "summary"):
        print(f"--- {kind} ---")
        merge(kind)
    print(f"\nMerged CSVs written to {OUT}")


if __name__ == "__main__":
    main()
