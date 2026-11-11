"""Regression checks for model refreshes and operational capability estimates."""
import unittest
from estimate_rungs import add_coding


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
