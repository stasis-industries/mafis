"""
analyze_tails.py  —  CVaR (alpha=0.8) + TEP on all_runs.csv
Output: results/tails.csv
"""
import pandas as pd
import numpy as np

# ---------------------------------------------------------------------------
# Load
# ---------------------------------------------------------------------------
df = pd.read_csv("results/all_runs.csv")
faulted = df[df["is_baseline"] == False].copy()

# ---------------------------------------------------------------------------
# CVaR helper
# ---------------------------------------------------------------------------
def cvar(values, alpha=0.8, worst_is_low=True):
    """
    CVaR_alpha = mean of worst (1-alpha)*N values.
    worst_is_low=True  → low values are bad (FT: low = bad).
    worst_is_low=False → high (positive) values are bad (deficit: large negative = bad).
    Returns (cvar, ci95_lo, ci95_hi) via bootstrap n=1000.
    """
    v = np.asarray([x for x in values if pd.notna(x)])
    if len(v) == 0:
        return (np.nan, np.nan, np.nan)
    k = max(1, int(np.ceil((1 - alpha) * len(v))))
    worst = np.sort(v)[:k] if worst_is_low else np.sort(v)[-k:]
    point = worst.mean()
    # Bootstrap CI
    rng = np.random.default_rng(42)
    boot = []
    for _ in range(1000):
        sample = rng.choice(v, size=len(v), replace=True)
        worst_s = np.sort(sample)[:k] if worst_is_low else np.sort(sample)[-k:]
        boot.append(worst_s.mean())
    ci_lo, ci_hi = np.percentile(boot, [2.5, 97.5])
    return (point, ci_lo, ci_hi)

# ---------------------------------------------------------------------------
# TEP helpers
# ---------------------------------------------------------------------------
def tep_survival(group):
    return (group["survival_rate"] < 0.5).mean()

def tep_ft(group):
    return (group["fault_tolerance"] < 0.3).mean()

def tep_zero_tasks(group):
    return (group["total_tasks"] == 0).mean()

# ---------------------------------------------------------------------------
# Group + aggregate
# ---------------------------------------------------------------------------
gkeys = ["solver", "topology", "scenario", "num_agents"]
rows = []

for keys, group in faulted.groupby(gkeys):
    ft_vals = group["fault_tolerance"].dropna()
    def_vals = group["deficit_integral"].dropna()
    ar_vals = group["attack_rate"].dropna()

    ft_cvar, ft_lo, ft_hi = cvar(ft_vals, alpha=0.8, worst_is_low=True)
    def_cvar, def_lo, def_hi = cvar(def_vals, alpha=0.8, worst_is_low=False)
    ar_cvar, ar_lo, ar_hi = cvar(ar_vals, alpha=0.8, worst_is_low=False)

    tep_itae_high = (group["itae"] > 50000).mean()
    tep_ar_gt_05 = (group["attack_rate"] > 0.5).mean()

    rows.append({
        **dict(zip(gkeys, keys)),
        "n_seeds": len(group),
        "ft_cvar08": ft_cvar,
        "ft_cvar08_lo": ft_lo,
        "ft_cvar08_hi": ft_hi,
        "deficit_cvar08": def_cvar,
        "deficit_cvar08_lo": def_lo,
        "deficit_cvar08_hi": def_hi,
        "ar_cvar08": ar_cvar,
        "ar_cvar08_lo": ar_lo,
        "ar_cvar08_hi": ar_hi,
        "tep_survival_lt_05": tep_survival(group),
        "tep_ft_lt_03": tep_ft(group),
        "tep_zero_tasks": tep_zero_tasks(group),
        "tep_itae_high": tep_itae_high,
        "tep_ar_gt_05": tep_ar_gt_05,
    })

out = pd.DataFrame(rows)
out.to_csv("results/tails.csv", index=False)
print(f"Written {len(out)} rows to results/tails.csv")
print(out.head(10).to_string())
