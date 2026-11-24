#!/usr/bin/env python3
"""Write the rung estimates file, crates/lucidos-core/src/llm/rung_estimates.json.

Usage: estimate_rungs.py <inputs-dir> [--print]

The inputs directory is docs/plans/rung-estimate-inputs, which the release
drops, because Artificial Analysis (AA) forbids republishing its figures.
Only order, pair identities, frontier flags and coarse bands ship. It holds:

- aa_rungs.csv: model,effort,score,cost_usd,time_s,output_tokens_m,
  output_speed_tps, read off AA's model pages. A pair needs a score, a cost
  and its output tokens. time_s and output_speed_tps may be empty.
- aa_coding.csv: model,effort,score, AA's Coding Agent Index (0 to 1), read
  off https://artificialanalysis.ai/agents/coding-agents. Pairs it lacks are
  left out.
- openrouter_speed.csv: model,throughput_tps, standard tier only. A model it
  lacks takes AA's speed instead.

Output prices come from the shipped price table, PRICE_TABLE: the card in
force today. The engine prices the token band from the same table.

Every pair carries two intelligence profiles:

- general: AA's Intelligence Index as measured. Every pair comes from one
  AA snapshot, so no second source is blended in.
- coding: AA's Coding Agent Index, scaled onto the general scale by matching
  mean and spread over the pairs both cover. A pair the index lacks takes its
  general score plus its model's mean measured gap (coding minus general), and
  is flagged coding_estimated. A model with no measured pair has gap zero.
  CODING_UNMEASURED names models whose lone index row is ignored.

Both capability profiles use a cumulative maximum in effort order before
ranking or frontier selection. Downward observations emit review diagnostics;
ties are estimates, not evidence of strict improvement. Raw inputs stay intact.
coding_estimated indicates missing index coverage, not an unsmoothed score.

Cost is AA's measured cost per task: a pair AA has not costed is skipped.
Time blends AA at AA_WEIGHT with an independent view: output tokens over the
model's speed, calibrated against AA's times. The view's fit runs over
FIT_MODELS only, so adding a model never moves another.

Bands count steps from REFERENCE: the token band COST_STEPS_PER_DOUBLING to a
doubling, the time band TIME_STEPS_PER_DOUBLING. The token band is cost per
task over the model's output price: billed tokens in output-token equivalents.
No cost band ships: the engine multiplies the token band by the price table's
output price, so cost stays right when a price changes. Cost still ranks the
pairs. Each frontier uses the shipped cost. An intelligence band is a score in
steps of SCORE_POINTS_PER_STEP, rounded half up, never the score itself.

Each surface lists its own pairs, ranked on its profile. A pair is on when no
other pair of that surface beats it on score and cost, where cost is the
shipped token band plus the price, as the engine computes it. A pair the
coding index never measured scores its general score there. Each surface's
SHIPPED_DEFAULTS pair stays on even when another pair beats it, so a fresh
workspace's default is a rung of its ladder. Every pair also names every
pair that dominates it, so runtime can recompute the frontier for available
routes without raw scores. --print shows the estimates
instead of writing.
"""

import csv
import json
import math
import statistics
import sys
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
TARGET = ROOT / "crates/lucidos-core/src/llm/rung_estimates.json"
PRICE_TABLE = ROOT / "crates/lucidos-core/src/llm/model_prices.json"

SOURCE = (
    "Lucidos estimate from Artificial Analysis (artificialanalysis.ai): its "
    "Intelligence Index, Coding Agent Index and cost per task, with list "
    "prices and OpenRouter speed. "
    "Bands only: see estimate_rungs.py in the update-lucidos-models skill."
)

AA_WEIGHT = 0.75
CODING_EFFORTS = ["low", "medium", "high", "xhigh", "max"]
REFERENCE = ("claude-opus-5-5", "medium")
# Each surface's default on a fresh workspace. Engine tests fail when one drifts:
# the_chat_ladder_loads_with_the_account_default_marked and the_shipped_*_default_*.
SHIPPED_DEFAULTS = {
    "chat": REFERENCE,
    "claude_code": REFERENCE,
    "codex": ("gpt-6.2-sol", "medium"),
}
# Models whose coding index rows are ignored: the index measured them at one
# effort only, so they take general, and max reads as xhigh. docs/temporary-measures.md.
CODING_UNMEASURED = {"claude-opus-5-5"}
COST_STEPS_PER_DOUBLING = 6
TIME_STEPS_PER_DOUBLING = 3
SCORE_POINTS_PER_STEP = 2

# Every estimated model by its Lucidos id: its display name as AA's records
# give it, and the
# efforts each surface offers it at. Chat lists every builtin registry model
# that is not retired, at the tiers `llm::reasoning::supported_efforts` gives
# it on any route.
MODELS = {
    "claude-fable-5-2": ("Claude Fable 5.2", {
        "chat": CODING_EFFORTS, "claude_code": CODING_EFFORTS}),
    "gemini-4-argon": ("Gemini 4 Argon", {"chat": ["low", "medium", "high"]}),
    "gemini-4-carbon": ("Gemini 4 Carbon", {"chat": ["low", "medium", "high"]}),
    "gemini-4-neon": ("Gemini 4 Neon", {"chat": ["low", "medium", "high", "xhigh"]}),
    "claude-haiku-5-5": ("Claude Haiku 5.5", {
        "chat": CODING_EFFORTS, "claude_code": CODING_EFFORTS}),
    "claude-sonnet-5-5": ("Claude Sonnet 5.5", {
        "chat": CODING_EFFORTS, "claude_code": CODING_EFFORTS}),
    "claude-opus-5-5": ("Claude Opus 5.5", {
        "chat": CODING_EFFORTS, "claude_code": CODING_EFFORTS}),
    "gpt-6.2-astra": ("GPT-6.2 Astra", {"chat": CODING_EFFORTS, "codex": CODING_EFFORTS}),
    "gpt-6.2-sol": ("GPT-6.2 Sol", {"chat": CODING_EFFORTS, "codex": CODING_EFFORTS}),
    "gpt-6.1-sol": ("GPT-6.1 Sol", {"chat": CODING_EFFORTS, "codex": CODING_EFFORTS}),
    "gpt-6-astra": ("GPT-6 Astra", {
        "chat": ["none"] + CODING_EFFORTS, "codex": CODING_EFFORTS}),
    "gpt-6-sol": ("GPT-6 Sol", {"codex": CODING_EFFORTS}),
    "gpt-6-luna": ("GPT-6 Luna", {
        "chat": ["none", "low", "medium", "high", "xhigh"], "codex": CODING_EFFORTS}),
    "gpt-5.6-sol": ("GPT-5.6 Sol", {"codex": CODING_EFFORTS}),
    "gpt-5.6-terra": ("GPT-5.6 Terra", {"codex": CODING_EFFORTS}),
    "gpt-5.6-luna": ("GPT-5.6 Luna", {"codex": CODING_EFFORTS}),
    "gemini-3.1-pro-preview": ("Gemini 3.1 Pro", {"chat": ["none", "low", "medium", "high"]}),
    "gemini-3.8-flash": ("Gemini 3.8 Flash", {"chat": ["low", "medium", "high"]}),
    "gemini-3.1-flash-lite": ("Gemini 3.1 Flash-Lite", {"chat": ["none", "low", "medium", "high"]}),
    "z-ai/glm-5.2": ("GLM-5.2", {"chat": ["none", "low", "medium", "high"]}),
    "grok-4.6": ("Grok 4.6", {"chat": ["none", "low", "medium", "high"]}),
    "grok-4.20": ("Grok 4.20", {"chat": ["none", "low", "medium", "high"]}),
}

SURFACES = {"chat": "general", "claude_code": "coding", "codex": "coding"}

# The models the independent view fits against: AA's current index measures
# each at every coding effort.
FIT_MODELS = [
    "claude-haiku-5-5", "claude-sonnet-5-5", "claude-opus-5-5",
    "gpt-6.1-sol", "gpt-6-astra", "gpt-6-luna",
]

# Keep the established coding scale fixed on additive model releases, just as
# FIT_MODELS anchors the independent view. A deliberate full recalibration can
# revise this cohort; a newly measured model must not rescale existing bands.
CODING_SCALE_MODELS = set(FIT_MODELS) | {"gemini-3.8-flash"}


def read_csv(path):
    with open(path, newline="") as f:
        return list(csv.DictReader(f))


def number(text):
    return float(text) if text not in ("", None) else None


def slope(xs, ys):
    mx, my = statistics.mean(xs), statistics.mean(ys)
    return sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / sum((x - mx) ** 2 for x in xs)


def two_figures(value):
    return float(f"{value:.2g}")


def observation_key(row, seen, source, efforts_only=False):
    model, effort = row["model"], row["effort"]
    if model not in MODELS or effort not in {e for es in MODELS[model][1].values() for e in es}:
        raise ValueError(f"{source}: unknown model/effort {model}={effort}")
    key = (model, effort)
    if (effort if efforts_only else key) in seen:
        raise ValueError(f"{source}: duplicate observation {model}={effort}")
    return key


def coding_observations(rows):
    found = {}
    for row in rows:
        key = observation_key(row, found, "aa_coding.csv")
        score = float(row["score"])
        if not math.isfinite(score) or not 0 <= score <= 1:
            raise ValueError(f"aa_coding.csv: {key[0]}={key[1]} needs a finite score in [0, 1]")
        found[key] = score
    return found


def coding_pair(record):
    model = record.get("model")
    if not isinstance(model, dict):
        return None
    release, effort = model.get("release"), model.get("effort")
    if not isinstance(release, dict) or not isinstance(effort, dict):
        return None
    if not isinstance(effort.get("slug"), str):
        return None
    for identifier, (name, surfaces) in MODELS.items():
        if release.get("shortName") == name and effort.get("slug") in {
                e for efforts in surfaces.values() for e in efforts}:
            return identifier, effort["slug"]
    return None


def evaluation_cohort(record):
    evaluations = record.get("evals", [])
    if not evaluations:
        raise ValueError("coding provenance: missing evaluation cohort")
    cohort = []
    for evaluation in evaluations:
        fields = ("datasetIndexName", "evaluationDatasetSlug", "refDatasetName", "weight")
        if any(not evaluation.get(field) for field in fields):
            raise ValueError("coding provenance: unidentified evaluation cohort")
        cohort.append({field: evaluation[field] for field in fields})
    names = [evaluation["datasetIndexName"] for evaluation in cohort]
    if len(set(names)) != len(names) or not math.isclose(sum(e["weight"] for e in cohort), 1):
        raise ValueError("coding provenance: invalid evaluation weights or duplicate datasets")
    return sorted(cohort, key=lambda e: e["datasetIndexName"])


def coding_snapshot(records, source_url, extracted):
    observations, seen = [], set()
    for record in records:
        pair = coding_pair(record)
        if pair is None:
            continue
        if pair in seen:
            raise ValueError(f"coding provenance: duplicate observation {pair}")
        seen.add(pair)
        observations.append(record)
    if not observations:
        raise ValueError("coding provenance: no supported explicit model/effort observations")
    evidence = {"source_url": source_url, "extracted": extracted,
                "cohort": evaluation_cohort(observations[0]), "observations": observations}
    coding_evidence(evidence)
    return evidence


def coding_evidence(evidence):
    if not evidence.get("source_url") or not evidence.get("extracted") or not evidence.get("cohort"):
        raise ValueError("coding provenance: source URL, extraction date and comparable cohort are required")
    rows = []
    for record in evidence.get("observations", []):
        pair = coding_pair(record)
        if pair is None:
            raise ValueError("coding provenance: unidentified or unsupported model/effort")
        if evaluation_cohort(record) != evidence["cohort"]:
            raise ValueError("coding provenance: incompatible active evaluation cohort; refresh the complete snapshot")
        versions = record.get("versions", {})
        for evaluation in evidence["cohort"]:
            version = versions.get(evaluation["datasetIndexName"], {})
            if any(not version.get(bound, {}).get("version") for bound in ("min", "max")):
                raise ValueError("coding provenance: missing harness versions")
        if any(not record.get(field) for field in ("id", "displayLabel", "agentName")):
            raise ValueError("coding provenance: missing source identity or harness")
        rows.append({"model": pair[0], "effort": pair[1], "score": record["indexScore"]})
    if not rows:
        raise ValueError("coding provenance: empty active cohort")
    return coding_observations(rows)


def validated_coding(rows, evidence):
    observed, verified = coding_observations(rows), coding_evidence(evidence)
    if observed != verified:
        raise ValueError("aa_coding.csv differs from coding provenance; reimport the coherent snapshot")
    return observed


def normalize_efforts(found):
    """Operational estimates use a monotone envelope, not raw benchmark ranks.

    Keep observations intact. A downward fluctuation is insufficient evidence
    to demote a higher effort; preserve ties without inventing strict gains.
    coding_estimated continues to describe missing coding-index coverage,
    rather than whether a covered score was smoothed.
    """
    for model in sorted({model for model, _ in found}):
        for profile in ("general", "coding"):
            previous = None
            for effort in ["none"] + CODING_EFFORTS:
                row = found.get((model, effort))
                if row is None:
                    continue
                score = row[profile]
                if previous is not None and score < previous:
                    print(f"normalized {model}={effort} {profile}: downward effort inversion; "
                          "using the preceding effort estimate (review source variant/harness)",
                          file=sys.stderr)
                    row[profile] = previous
                previous = row[profile]


class Inputs:
    def __init__(self, folder):
        folder = Path(folder)
        self.aa = {}
        for row in read_csv(folder / "aa_rungs.csv"):
            if row["model"] not in MODELS:
                sys.exit(f"aa_rungs.csv names {row['model']}, which MODELS lacks")
            key = observation_key(row, self.aa.get(row["model"], {}), "aa_rungs.csv", efforts_only=True)
            self.aa.setdefault(key[0], {})[key[1]] = {
                key: number(row[key])
                for key in ("score", "cost_usd", "time_s", "output_tokens_m", "output_speed_tps")
            }
        provenance = folder / "coding-provenance.json"
        if not provenance.exists():
            raise ValueError("missing coding-provenance.json; import a coherent coding snapshot before calibration")
        self.coding = validated_coding(read_csv(folder / "aa_coding.csv"), json.loads(provenance.read_text()))
        self.price_out = output_prices_today()
        unpriced = sorted({m for m, e in self.pairs() if self.estimable(m, e)}
                          - set(self.price_out))
        if unpriced:
            sys.exit(f"{PRICE_TABLE.name} has no card for {', '.join(unpriced)}")
        self.speed = {row["model"]: float(row["throughput_tps"])
                      for row in read_csv(folder / "openrouter_speed.csv")}

    def pairs(self):
        return [(m, e) for m, rows in self.aa.items() for e in rows]

    def fit_rows(self, model):
        """The model's rows at the coding efforts, which every fit reads."""
        return {e: r for e, r in self.aa[model].items() if e in CODING_EFFORTS}

    def tokens(self, model, effort):
        return self.aa[model][effort]["output_tokens_m"] * 1e6

    def aa_speed(self, model, effort):
        """AA's speed for the pair, else the model's median across efforts."""
        return self.aa[model][effort]["output_speed_tps"] or statistics.median(
            r["output_speed_tps"] for r in self.fit_rows(model).values() if r["output_speed_tps"])

    def estimable(self, model, effort):
        row = self.aa[model][effort]
        return all(row[k] is not None for k in ("score", "cost_usd", "output_tokens_m"))


def output_prices_today():
    """Each model's output rate on its newest card from today or before,
    the rule `PriceTable::card_on` applies."""
    today = date.today().isoformat()
    prices = {}
    for model, cards in json.loads(PRICE_TABLE.read_text())["models"].items():
        in_force = [c for c in cards if c.get("from", "") <= today]
        if in_force:
            prices[model] = in_force[-1]["out"]
    return prices


class IndependentView:
    """The non-AA estimate of time uses an aggregate fit, not a single pair's
    figure. FIT_MODELS anchors it, so adding another model never moves it.
    """

    def __init__(self, inputs):
        self.inputs = inputs
        aa = inputs.aa
        timed = [(m, e) for m in FIT_MODELS for e in CODING_EFFORTS if aa[m][e]["time_s"]]
        xs = [inputs.tokens(m, e) / inputs.aa_speed(m, e) for m, e in timed]
        ys = [aa[m][e]["time_s"] for m, e in timed]
        self.seconds_per_second = slope(xs, ys)
        self.base_seconds = statistics.mean(ys) - self.seconds_per_second * statistics.mean(xs)

    def time(self, model, effort):
        speed = self.inputs.speed.get(model) or self.inputs.aa_speed(model, effort)
        seconds = self.inputs.tokens(model, effort) / speed
        return self.seconds_per_second * seconds + self.base_seconds


def blend(published, independent):
    if independent is None:
        return published
    return AA_WEIGHT * published + (1 - AA_WEIGHT) * independent


def estimates(inputs):
    view = IndependentView(inputs)
    out = {}
    for model, rows in inputs.aa.items():
        for effort, aa in rows.items():
            if not inputs.estimable(model, effort):
                print(f"skipped {model}={effort}: AA publishes no cost or tokens", file=sys.stderr)
                continue
            time = view.time(model, effort)
            out[(model, effort)] = {
                "model": model,
                "effort": effort,
                "general": round(aa["score"]),
                "cost": two_figures(aa["cost_usd"]),
                "time": two_figures(blend(aa["time_s"], time) if aa["time_s"] else time),
            }
    add_coding(out, inputs.coding)
    normalize_efforts(out)
    for row in out.values():
        row["shipped_cost"] = shipped_cost(row, out[REFERENCE], inputs.price_out)
    return out


def add_coding(found, coding):
    """Scale the Coding Agent Index onto the general scale, then fill each
    pair's coding score. A pair the index lacks adds its model's mean measured
    gap to its general score, so one model's pairs stay on one scale."""
    coding = {key: v for key, v in coding.items() if key[0] not in CODING_UNMEASURED}
    both = [(coding[key], row["general"]) for key, row in found.items()
            if key in coding and key[0] in CODING_SCALE_MODELS]
    if (len(both) < 2 or len({c for c, _ in both}) < 2
            or len({g for _, g in both}) < 2):
        raise ValueError("no comparable coding calibration overlap; import measured anchor pairs with nonzero spread")
    mean_c, mean_g = (statistics.mean(v) for v in zip(*both))
    spread = statistics.stdev(g for _, g in both) / statistics.stdev(c for c, _ in both)

    def scaled(key):
        return mean_g + (coding[key] - mean_c) * spread

    gaps = {}
    for key, row in found.items():
        if key in coding:
            gaps.setdefault(key[0], []).append(scaled(key) - row["general"])
    for key, row in found.items():
        measured = key in coding
        row["coding_estimated"] = not measured
        if measured:
            row["coding"] = round(scaled(key))
        else:
            row["coding"] = round(row["general"] + statistics.mean(gaps.get(key[0], [0])))
    for model in CODING_UNMEASURED:
        if (model, "max") in found and (model, "xhigh") in found:
            found[(model, "max")]["coding"] = found[(model, "xhigh")]["coding"]


def intelligence_band(score):
    """Half up, so 55 and 57 stay apart where round() would make both 28."""
    return math.floor(score / SCORE_POINTS_PER_STEP + 0.5)


def steps(value, reference, per_doubling):
    return round(per_doubling * math.log2(value / reference))


def frontier_score(row, profile):
    """A pair the coding index never measured ranks on its general score here.
    One measured point of its model must not turn the model's whole range off."""
    return row["general"] if row["coding_estimated"] else row[profile]


def beats(a, b, profile):
    """`a` is no worse than `b` on score and cost, and better on one."""
    score_a, score_b = frontier_score(a, profile), frontier_score(b, profile)
    return (score_a >= score_b and a["shipped_cost"] <= b["shipped_cost"]
            and (score_a > score_b or a["shipped_cost"] < b["shipped_cost"]))


def ranked(rows, profile):
    return sorted(rows, key=lambda r: (r[profile], r["cost"], r["time"]))


def token_band(row, reference, price_out):
    tokens = row["cost"] / price_out[row["model"]]
    reference_tokens = reference["cost"] / price_out[reference["model"]]
    return steps(tokens, reference_tokens, COST_STEPS_PER_DOUBLING)


def shipped_cost(row, reference, price_out):
    """Cost in steps as the engine computes it: the rounded token band plus the
    output price. A frontier on exact cost can disagree with the shipped bands.
    Rounded so that equal costs compare equal: Sonnet at $10 plus 4 steps must
    tie Opus at $20 plus 1, not differ in the last float bit."""
    steps_cost = token_band(row, reference, price_out) + COST_STEPS_PER_DOUBLING * math.log2(price_out[row["model"]])
    return round(steps_cost, 9)


def bands(row, reference, price_out):
    return {
        "model": row["model"],
        "effort": row["effort"],
        "general_band": intelligence_band(row["general"]),
        "coding_band": intelligence_band(row["coding"]),
        "coding_estimated": row["coding_estimated"],
        "token_band": token_band(row, reference, price_out),
        "time_band": steps(row["time"], reference["time"], TIME_STEPS_PER_DOUBLING),
    }


def surface(found, name):
    profile = SURFACES[name]
    rows = [row for (model, effort), row in found.items()
            if effort in MODELS[model][1].get(name, [])]
    return [{"model": r["model"], "effort": r["effort"],
             "on": (r["model"], r["effort"]) == SHIPPED_DEFAULTS[name]
             or not any(beats(other, r, profile) for other in rows),
             "pinned": (r["model"], r["effort"]) == SHIPPED_DEFAULTS[name],
             "dominators": [{"model": other["model"], "effort": other["effort"]}
                            for other in rows if beats(other, r, profile)]}
            for r in ranked(rows, profile)]


def one_per_line(items, indent):
    pad = " " * indent
    return ",\n".join(f"{pad}{{ {json.dumps(i, separators=(', ', ': '))[1:-1]} }}" for i in items)


def render(pairs, surfaces):
    """The file in its own layout: one pair per line."""
    blocks = ",\n".join(
        f'    "{name}": [\n{one_per_line(entries, 6)}\n    ]' for name, entries in surfaces.items())
    note = (
        "The rung estimates: bands for every estimated (model, effort) pair, "
        "and each ladder surface's shipped pairs, ranked on its profile, with "
        "global frontier flags, pinned defaults and all dominating pair identities "
        "(runtime/intelligence_ladder.rs). Bands only, "
        "never a third-party figure (ADR 0500, ADR 0525). estimate_rungs.py "
        "in the update-lucidos-models skill writes it, so never edit it by hand."
    )
    return (
        "{\n"
        f'  "_note": {json.dumps(note)},\n'
        f'  "source": {json.dumps(SOURCE + " Capability estimates use a nondecreasing effort envelope; covered coding scores may be smoothed.")},\n'
        f'  "estimated": "{date.today().isoformat()}",\n'
        '  "pairs": [\n'
        f"{one_per_line(pairs, 4)}\n"
        "  ],\n"
        '  "surfaces": {\n'
        f"{blocks}\n"
        "  }\n"
        "}\n"
    )


def main():
    args = sys.argv[1:]
    if not args or len(args) > 2 or args[1:] not in ([], ["--print"]):
        sys.exit(__doc__)
    inputs = Inputs(args[0])
    found = estimates(inputs)
    if args[1:] == ["--print"]:
        for row in ranked(found.values(), "general"):
            print(f"{row['model']:24} {row['effort']:6} {row['general']:3} {row['coding']:3}"
                  f"{'*' if row['coding_estimated'] else ' '} {row['cost']:8g} {row['time']:6g}")
        return
    reference = found[REFERENCE]
    pairs = [bands(r, reference, inputs.price_out) for r in ranked(found.values(), "general")]
    surfaces = {name: surface(found, name) for name in SURFACES}
    TARGET.write_text(render(pairs, surfaces))
    json.loads(TARGET.read_text())


if __name__ == "__main__":
    main()
