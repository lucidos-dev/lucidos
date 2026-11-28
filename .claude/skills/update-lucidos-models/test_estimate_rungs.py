"""Regression checks for model refreshes and operational capability estimates."""
import unittest
import contextlib
import copy
import io
import json
import estimate_rungs as rungs
from estimate_rungs import add_coding


@unittest.skipUnless((rungs.ROOT / "docs/plans/rung-estimate-inputs/aa_rungs.csv").exists(),
                     "AA inputs are private and excluded from the public release")
class MeasuredCostTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.inputs = rungs.Inputs(rungs.ROOT / "docs/plans/rung-estimate-inputs")
        with contextlib.redirect_stderr(io.StringIO()):
            cls.found = rungs.estimates(cls.inputs)

    def test_every_cost_is_the_measured_cost(self):
        for key, row in self.found.items():
            measured = self.inputs.aa[key[0]][key[1]]["cost_usd"]
            with self.subTest(pair=key):
                self.assertEqual(row["cost"], rungs.two_figures(measured))

    def test_full_roster_subset_dominance_parity(self):
        import random
        randomizer = random.Random(20261117)
        for name, profile in rungs.SURFACES.items():
            metadata = rungs.surface(self.found, name)
            for _ in range(100):
                available = {(p["model"], p["effort"]) for p in metadata
                             if randomizer.choice([True, False])}
                actual = {(p["model"], p["effort"]) for p in metadata
                          if (p["model"], p["effort"]) in available
                          and (p["pinned"] or not any(
                              (d["model"], d["effort"]) in available for d in p["dominators"]))}
                expected = {key for key in available if key == rungs.SHIPPED_DEFAULTS[name]
                            or not any(rungs.beats(self.found[other], self.found[key], profile)
                                       for other in available)}
                self.assertEqual(actual, expected, name)

    def test_shipped_fable_medium_ratio_respects_measured_ratio_and_rounding(self):
        shipped = {(r["model"], r["effort"]): r
                   for r in json.loads(rungs.TARGET.read_text())["pairs"]}
        fable, opus = ("claude-fable-5-2", "medium"), ("claude-opus-5-5", "xhigh")
        ratio = (2 ** ((shipped[fable]["token_band"] - shipped[opus]["token_band"])
                       / rungs.COST_STEPS_PER_DOUBLING)
                 * self.inputs.price_out[fable[0]] / self.inputs.price_out[opus[0]])
        measured_ratio = (self.inputs.aa[fable[0]][fable[1]]["cost_usd"]
                          / self.inputs.aa[opus[0]][opus[1]]["cost_usd"])
        # Each rounded band contributes at most half a step to the ratio.
        self.assertLessEqual(ratio, measured_ratio * 2 ** (1 / rungs.COST_STEPS_PER_DOUBLING))
        self.assertGreaterEqual(ratio, measured_ratio / 2 ** (1 / rungs.COST_STEPS_PER_DOUBLING))

    def test_adding_a_non_anchor_model_preserves_existing_bands(self):
        inputs = copy.deepcopy(self.inputs)
        model = "additional-model"
        inputs.aa[model] = copy.deepcopy(inputs.aa["claude-fable-5-2"])
        inputs.price_out[model] = inputs.price_out["claude-fable-5-2"]
        with contextlib.redirect_stderr(io.StringIO()):
            after = rungs.estimates(inputs)
        for key, row in self.found.items():
            with self.subTest(pair=key):
                self.assertEqual(rungs.bands(row, self.found[rungs.REFERENCE], inputs.price_out),
                                 rungs.bands(after[key], after[rungs.REFERENCE], inputs.price_out))

    def test_coding_refresh_preserves_general_token_and_time_bands(self):
        inputs = copy.deepcopy(self.inputs)
        inputs.coding = rungs.coding_observations(rungs.read_csv(
            rungs.ROOT / "docs/plans/rung-estimate-inputs/aa_coding-historical.csv"))
        with contextlib.redirect_stderr(io.StringIO()):
            historical = rungs.estimates(inputs)
        for key, current in self.found.items():
            before = rungs.bands(historical[key], historical[rungs.REFERENCE], inputs.price_out)
            after = rungs.bands(current, self.found[rungs.REFERENCE], inputs.price_out)
            with self.subTest(pair=key):
                for field in ("general_band", "token_band", "time_band"):
                    self.assertEqual(before[field], after[field], field)


class CodingScaleTests(unittest.TestCase):
    def test_a_new_measurement_does_not_rescale_existing_models(self):
        anchors = {
            ("claude-haiku-5-5", "high"): {"general": 10},
            ("claude-sonnet-5-5", "high"): {"general": 20},
        }
        coding = {("claude-haiku-5-5", "high"): 0.1,
                  ("claude-sonnet-5-5", "high"): 0.2}
        before = {key: dict(row) for key, row in anchors.items()}
        add_coding(before, coding)
        after = {key: dict(row) for key, row in anchors.items()}
        after[("gemini-4-carbon", "high")] = {"general": 100}
        add_coding(after, {**coding, ("gemini-4-carbon", "high"): 0.15})
        for key in anchors:
            self.assertEqual(before[key], after[key])
        self.assertFalse(after[("gemini-4-carbon", "high")]["coding_estimated"])
        self.assertEqual(after[("gemini-4-carbon", "high")]["coding"], 15)

    def test_missing_effort_uses_estimate_without_copying_measurement(self):
        found = {("claude-haiku-5-5", "high"): {"general": 10},
                 ("claude-sonnet-5-5", "high"): {"general": 20},
                 ("claude-haiku-5-5", "low"): {"general": 5}}
        coding = {("claude-haiku-5-5", "high"): 0.1,
                  ("claude-sonnet-5-5", "high"): 0.2}
        original = dict(coding)
        add_coding(found, coding)
        self.assertEqual(coding, original)
        self.assertTrue(found[("claude-haiku-5-5", "low")]["coding_estimated"])
        self.assertEqual(found[("claude-haiku-5-5", "low")]["coding"], 5)



class InputGuardTests(unittest.TestCase):
    def evidence(self):
        record = {
            "id": "fixture", "displayLabel": "Fixture agent - GPT-6.1 Sol (High)",
            "agentName": "Fixture agent", "indexScore": 0.6,
            "model": {"release": {"shortName": "GPT-6.1 Sol"},
                      "effort": {"slug": "high"}},
            "evals": [{"datasetIndexName": "fixture-v1", "refDatasetName": "fixture@v1",
                       "evaluationDatasetSlug": "Fixture v1", "weight": 1}],
            "versions": {"fixture-v1": {"min": {"version": "1"}, "max": {"version": "1"}}},
        }
        return rungs.coding_snapshot([record], "https://example.com/coding", "2026-11-17")

    def test_active_coding_requires_comparable_evidence(self):
        evidence = self.evidence()
        rows = [{"model": "gpt-6.1-sol", "effort": "high", "score": "0.6"}]
        self.assertEqual(rungs.validated_coding(rows, evidence),
                         {("gpt-6.1-sol", "high"): 0.6})
        for mutation in ("cohort", "evals", "versions", "effort", "score", "duplicate"):
            bad = copy.deepcopy(evidence)
            record = bad["observations"][0]
            if mutation == "cohort":
                bad.pop("cohort")
            elif mutation == "evals":
                record["evals"][0]["refDatasetName"] = "incompatible-v2"
            elif mutation == "versions":
                record["versions"] = {}
            elif mutation == "effort":
                record["model"]["effort"] = "$undefined"
            elif mutation == "score":
                record["indexScore"] = 0.7
            else:
                bad["observations"].append(copy.deepcopy(record))
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                rungs.validated_coding(rows, bad)

    def test_snapshot_skips_unidentified_and_unrelated_models(self):
        evidence = self.evidence()
        records = evidence["observations"]
        self.assertEqual(rungs.coding_snapshot(
            [*records, {"model": None}, {"model": "$undefined"},
             {"model": {"release": {"shortName": "GPT-6.1 Sol"}, "effort": "$undefined"}}],
            "https://example.com/coding", "2026-11-17"), evidence)
        with self.assertRaisesRegex(ValueError, "duplicate"):
            rungs.coding_snapshot(records * 2, "https://example.com/coding", "2026-11-17")

    def test_missing_calibration_overlap_fails_actionably(self):
        with self.assertRaisesRegex(ValueError, "comparable coding calibration"):
            add_coding({("gpt-6.1-sol", "high"): {"general": 20}}, {})

    def test_general_observations_reject_duplicate_efforts(self):
        from estimate_rungs import observation_key
        with self.assertRaisesRegex(ValueError, "duplicate"):
            observation_key({"model": "gpt-6.1-sol", "effort": "high"},
                            {"high": {}}, "aa_rungs.csv", efforts_only=True)

    def test_rejects_bad_coding_observations(self):
        from estimate_rungs import coding_observations
        good = {"model": "gpt-6.1-sol", "effort": "high", "score": "0.6"}
        self.assertEqual(coding_observations([good]), {("gpt-6.1-sol", "high"): 0.6})
        for rows in ([good, good], [{**good, "effort": "typo"}],
                     [{**good, "model": "unknown"}],
                     *[[{**good, "score": score}] for score in ("nan", "inf", "-0.1", "1.1")]):
            with self.subTest(rows=rows), self.assertRaises(ValueError):
                coding_observations(rows)


class EffortOrderTests(unittest.TestCase):
    def test_normalization_preserves_ties_and_other_models(self):
        from estimate_rungs import normalize_efforts
        found = {(model, effort): {"general": g, "coding": c}
                 for model, effort, g, c in [
                     ("a", "max", 54, 50), ("a", "medium", 48, 52),
                     ("a", "high", 50, 50), ("a", "xhigh", 52, 52),
                     ("b", "low", 20, 20), ("b", "high", 30, 30)]}
        normalize_efforts(found)
        self.assertEqual([found[("a", e)]["coding"] for e in
                          ("medium", "high", "xhigh", "max")], [52, 52, 52, 52])
        self.assertEqual(found[("b", "high")], {"general": 30, "coding": 30})
        self.assertEqual(found[("a", "max")]["general"], 54)

    def test_none_and_missing_efforts_and_general_inversions(self):
        from estimate_rungs import normalize_efforts
        found = {("a", "none"): {"general": 25, "coding": 20},
                 ("a", "high"): {"general": 20, "coding": 30}}
        normalize_efforts(found)
        self.assertEqual(found[("a", "high")], {"general": 25, "coding": 30})


class DominanceTests(unittest.TestCase):
    def test_all_subsets_match_direct_frontiers_with_ties_and_chains(self):
        # The middle point dominates the last even when the global winner is absent.
        model = "gpt-6.1-sol"
        rows = {}
        for effort, score, cost in [("low", 4, 1), ("medium", 3, 2),
                                    ("high", 2, 3), ("xhigh", 4, 1), ("max", 5, 4)]:
            rows[(model, effort)] = dict(model=model, effort=effort, general=score,
                                         coding=score, coding_estimated=False,
                                         shipped_cost=cost, cost=cost, time=1)
        metadata = rungs.surface(rows, "chat")
        for mask in range(1 << len(rows)):
            available = {key for i, key in enumerate(rows) if mask & (1 << i)}
            actual = {(p["model"], p["effort"]) for p in metadata
                      if (p["model"], p["effort"]) in available
                      and (p["pinned"] or not any(
                          (d["model"], d["effort"]) in available for d in p["dominators"]))}
            expected = {key for key in available if key == rungs.SHIPPED_DEFAULTS["chat"]
                        or not any(rungs.beats(rows[other], rows[key], "general")
                                   for other in available)}
            self.assertEqual(actual, expected)


if __name__ == "__main__":
    unittest.main()
