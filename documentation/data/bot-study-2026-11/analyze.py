# /// script
# requires-python = ">=3.11"
# dependencies = ["pandas", "numpy", "scipy", "statsmodels", "scikit-learn", "matplotlib"]
# ///
"""Usage: uv run analyze.py bots.csv OUT_DIR

Brains against outcomes: correlations, a regression with round fixed effects, a forest's
permutation importance, and a paired model of the 1v1s."""
import sys
from pathlib import Path

import numpy as np
import pandas as pd
import statsmodels.formula.api as smf
from scipy import stats
from sklearn.ensemble import GradientBoostingRegressor
from sklearn.inspection import permutation_importance, partial_dependence
from sklearn.model_selection import GroupKFold, cross_val_predict

pd.set_option("display.width", 200)
pd.set_option("display.max_rows", 200)

frame = pd.read_csv(sys.argv[1])
if "skill" in frame:
    frame = frame[frame["skill"] == "varied"].reset_index(drop=True)
out = Path(sys.argv[2])
out.mkdir(parents=True, exist_ok=True)

traits = [c for c in frame.columns if c.startswith(("route.", "mind.")) and pd.api.types.is_numeric_dtype(frame[c])]
traits = [t for t in traits if frame[t].std() > 0]
frame["preference"] = frame["mind.aim.preference"]
z = frame.copy()
for t in traits:
    z[t] = (frame[t] - frame[t].mean()) / frame[t].std()
safe = {t: t.replace("route.", "r_").replace("mind.", "m_").replace(".", "_") for t in traits}
z = z.rename(columns=safe)
names = list(safe.values())

# Within a round every bot faces the same opponents, so the outcome is centered per round.
for outcome in ["kd_rate", "damage_rate", "kill_share", "placing"]:
    z[outcome + "_c"] = z[outcome] - z.groupby("file")[outcome].transform("mean")

report = []

def say(text=""):
    print(text)
    report.append(str(text))

say(f"{len(frame)} bot-rounds, {frame['file'].nunique()} rounds, {frame['brain'].nunique()} brains")
say(frame.groupby(["map", "players"]).agg(rounds=("file", "nunique"), bot_rounds=("brain", "count")).to_string())
say()

# Spearman of each trait against the centered outcome, pooled and per size.
rows = []
for t, n in safe.items():
    row = {"trait": t}
    for label, part in [("all", z)] + [(f"{p}p", z[z.players == p]) for p in sorted(z.players.unique())]:
        if len(part) < 12:
            continue
        rho, p = stats.spearmanr(part[n], part["kd_rate_c"])
        row[f"rho_{label}"] = round(rho, 3)
        row[f"p_{label}"] = round(p, 4)
    rows.append(row)
spear = pd.DataFrame(rows).sort_values("rho_all", key=abs, ascending=False)
say("Spearman, trait against kills minus deaths per minute (centered per round)")
say(spear.to_string(index=False))
say()

# OLS with every trait at once, round fixed effects, errors clustered by brain.
formula = "kd_rate ~ " + " + ".join(names) + " + C(preference) + C(file)"
fit = smf.ols(formula, data=z).fit(cov_type="cluster", cov_kwds={"groups": pd.factorize(z["brain"])[0]})
coef = pd.DataFrame({"coef": fit.params, "se": fit.bse, "p": fit.pvalues})
coef = coef[[not i.startswith("C(file)") and i != "Intercept" for i in coef.index]].sort_values("p")
say("OLS: kills minus deaths per minute on standardized traits, round fixed effects, clustered by brain")
say(f"R² within {fit.rsquared:.3f}, n={int(fit.nobs)}")
say(coef.round(4).to_string())
say()

# The same with placing (1 first, 0 last), comparable across round sizes.
fit_place = smf.ols("placing ~ " + " + ".join(names) + " + C(preference) + C(file)", data=z).fit(cov_type="cluster", cov_kwds={"groups": pd.factorize(z["brain"])[0]})
cp = pd.DataFrame({"coef": fit_place.params, "se": fit_place.bse, "p": fit_place.pvalues})
cp = cp[[not i.startswith("C(file)") and i != "Intercept" for i in cp.index]].sort_values("p")
say("OLS: placing on standardized traits")
say(cp.round(4).to_string())
say()

# Per round size, the strongest few.
for p in sorted(z.players.unique()):
    part = z[z.players == p]
    if len(part) - part["file"].nunique() < 3 * len(names):
        continue
    f = smf.ols("kd_rate ~ " + " + ".join(names) + " + C(file)", data=part).fit(cov_type="cluster", cov_kwds={"groups": pd.factorize(part["brain"])[0]})
    c = pd.DataFrame({"coef": f.params, "p": f.pvalues})
    c = c[[i in names for i in c.index]].sort_values("p").head(8)
    say(f"OLS within {p}-player rounds ({part['file'].nunique()} rounds): strongest eight")
    say(c.round(4).to_string())
    say()

# A boosted forest, scored out of sample by brain, for shape and interactions.
X = z[names + ["players"]].to_numpy()
y = z["kd_rate_c"].to_numpy()
groups = pd.factorize(z["brain"])[0]
model = GradientBoostingRegressor(n_estimators=300, max_depth=3, learning_rate=0.03, subsample=0.8, random_state=0)
splits = min(5, len(set(groups)))
predicted = cross_val_predict(model, X, y, groups=groups, cv=GroupKFold(n_splits=splits))
say(f"Boosted trees, out of sample by brain: R² {1 - ((y - predicted) ** 2).sum() / ((y - y.mean()) ** 2).sum():.3f}")
model.fit(X, y)
imp = permutation_importance(model, X, y, n_repeats=20, random_state=0)
importance = pd.Series(imp.importances_mean, index=names + ["players"]).sort_values(ascending=False)
say(importance.head(12).round(4).to_string())
say()

# Partial dependence of the top traits, in their own units.
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
top = [n for n in importance.index if n != "players"][:6]
fig, axes = plt.subplots(2, 3, figsize=(12, 7))
inverse = {v: k for k, v in safe.items()}
for ax, name in zip(axes.flat, top):
    i = names.index(name)
    pd_result = partial_dependence(model, X, [i], grid_resolution=20)
    grid = pd_result["grid_values"][0] * frame[inverse[name]].std() + frame[inverse[name]].mean()
    ax.plot(grid, pd_result["average"][0], color="#3b6fb6")
    ax.scatter(frame[inverse[name]], z["kd_rate_c"], s=4, alpha=0.15, color="#888")
    ax.set_ylim(np.percentile(z["kd_rate_c"], 2), np.percentile(z["kd_rate_c"], 98))
    ax.set_title(inverse[name], fontsize=9)
    ax.set_ylabel("Δ kills−deaths /min")
fig.tight_layout()
fig.savefig(out / "partial_dependence.png", dpi=120)

# 1v1s as paired contests: the chance the first bot out-frags the second, on trait differences.
duels = []
for file, part in frame[frame.players == 2].groupby("file"):
    if len(part) != 2:
        continue
    a, b = part.iloc[0], part.iloc[1]
    if a["frags"] == b["frags"]:
        continue
    row = {safe[t]: (a[t] - b[t]) / frame[t].std() for t in traits}
    row["a_won"] = float(a["frags"] > b["frags"])
    duels.append(row)
if len(duels) >= 15:
    d = pd.DataFrame(duels)
    say(f"1v1 duels decided: {len(d)}")
    rows = []
    for n in names:
        rho, p = stats.spearmanr(d[n], d["a_won"])
        rows.append({"trait": n, "rho": round(rho, 3), "p": round(p, 4)})
    say(pd.DataFrame(rows).sort_values("p").head(12).to_string(index=False))
    say()

# Skill composite: each bot's mean percentile on the traits BotSkill narrows, better end up.
better_high = ["m_aim_turn_rate", "m_aim_tracking", "m_reflexes_attempt", "m_drives_cover", "m_drives_patience"]
better_low = ["m_reaction", "m_aim_error", "m_aim_settle", "m_reflexes_timing", "m_drives_roam"]
present = [n for n in better_high + better_low if n in z]
z["skill_index"] = sum(z[n] if n in better_high else -z[n] for n in present) / len(present)
rho, p = stats.spearmanr(z["skill_index"], z["kd_rate_c"])
say(f"Skill index (mean of the {len(present)} narrowed traits, better end up) against centered kd_rate: rho {rho:.3f}, p {p:.2g}")
z["skill_bin"] = pd.qcut(z["skill_index"], 5, labels=["lowest", "low", "middle", "high", "highest"])
say(z.groupby("skill_bin", observed=True)[["kd_rate_c", "placing", "kill_share"]].mean().round(3).to_string())
say()

# Mechanisms: what each behavior does for the outcome, and which traits drive each behavior.
behaviors = [c for c in frame.columns if c.startswith(("want.", "held.", "picked."))] + ["kill_distance", "dealt_rate", "taken_rate"]
behaviors = [b for b in behaviors if b in z and z[b].notna().sum() > 50 and z[b].std() > 0]
for b in behaviors:
    z[b + "_c"] = z[b] - z.groupby("file")[b].transform("mean")
rows = []
for b in behaviors:
    part = z[[b + "_c", "kd_rate_c"]].dropna()
    rho, p = stats.spearmanr(part[b + "_c"], part["kd_rate_c"])
    rows.append({"behavior": b, "rho_vs_kd": round(rho, 3), "p": round(p, 4), "mean": round(frame[b].mean(), 3)})
say("Behavior (centered per round) against centered kd_rate")
say(pd.DataFrame(rows).sort_values("p").to_string(index=False))
say()
grid = pd.DataFrame(index=names, columns=behaviors, dtype=float)
for n in names:
    for b in behaviors:
        part = z[[n, b + "_c"]].dropna()
        grid.loc[n, b] = stats.spearmanr(part[n], part[b + "_c"])[0]
strong = grid.where(grid.abs() >= 0.2)
say("Trait against behavior, Spearman |rho| >= 0.2 only")
for n in names:
    hits = strong.loc[n].dropna().sort_values(key=abs, ascending=False)
    if len(hits):
        say(f"  {n}: " + ", ".join(f"{b} {v:+.2f}" for b, v in hits.items()))
say()

(out / "report.txt").write_text("\n".join(report))
z.to_csv(out / "standardized.csv", index=False)
