"""Regression checks for model refreshes and operational capability estimates."""
import unittest
import contextlib
import copy
import io
import json
from types import SimpleNamespace
import estimate_rungs as rungs
from estimate_rungs import add_coding


class InputCostFitTests(unittest.TestCase):
    def view(self, tokens, residuals):
        view = rungs.IndependentView.__new__(rungs.IndependentView)
        view.tasks = 1
        rows = {e: {"output_tokens_m": t, "cost_usd": t + residual}
                for e, t, residual in zip(rungs.CODING_EFFORTS, tokens, residuals)}
        view.inputs = SimpleNamespace(
            price_out={"model": 1}, fit_rows=lambda model: rows,
            tokens=lambda model, effort: rows[effort]["output_tokens_m"] * 1e6)
        return view

    def test_input_cost_tracks_output_work_across_efforts(self):
        view = self.view([1, 2, 4], [3, 5, 7])
        for effort, expected in zip(rungs.CODING_EFFORTS, [4, 7, 11]):
            self.assertAlmostEqual(view.cost("model", effort), expected)

    def test_fit_uses_all_rows_instead_of_copying_the_pair(self):
        view = self.view([1, 2, 4], [1, 4, 1])
        self.assertAlmostEqual(view.cost("model", "medium"), 4)

    def test_flat_tokens_and_single_row_use_mean_input_cost(self):
        for tokens, residuals, expected in [([2, 2, 2], [1, 4, 1], 4), ([2], [3], 5)]:
            with self.subTest(tokens=tokens):
                self.assertAlmostEqual(self.view(tokens, residuals).cost("model", "low"), expected)

    def test_two_rows_use_mean_instead_of_exact_interpolation(self):
        view = self.view([1, 2], [1, 5])
        self.assertEqual(view.cost("model", "low"), 4)
        self.assertEqual(view.cost("model", "medium"), 5)

    def test_negative_prediction_is_floored_after_fitting(self):
        view = self.view([1, 2, 3], [-3, 1, 2])
        self.assertEqual(view.cost("model", "low"), view.output_cost("model", "low"))


@unittest.skipUnless((rungs.ROOT / "docs/plans/rung-estimate-inputs/aa_rungs.csv").exists(),
                     "AA inputs are private and excluded from the public release")
class MeasuredCostTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.inputs = rungs.Inputs(rungs.ROOT / "docs/plans/rung-estimate-inputs")
        with contextlib.redirect_stderr(io.StringIO()):
            cls.found = rungs.estimates(cls.inputs)

    def test_blended_cost_stays_within_20_percent_of_every_measured_cost(self):
        # Bound distortion below the independent view's 25% blend weight.
        for key, row in self.found.items():
            measured = self.inputs.aa[key[0]][key[1]]["cost_usd"]
            with self.subTest(pair=key):
                self.assertLessEqual(abs(row["cost"] / measured - 1), 0.20)

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
        inputs.eci[model] = 200
        with contextlib.redirect_stderr(io.StringIO()):
            after = rungs.estimates(inputs)
        for key, row in self.found.items():
            with self.subTest(pair=key):
                self.assertEqual(rungs.bands(row, self.found[rungs.REFERENCE], inputs.price_out),
                                 rungs.bands(after[key], after[rungs.REFERENCE], inputs.price_out))


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



class InputGuardTests(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()
