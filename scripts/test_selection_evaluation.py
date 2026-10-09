import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("evaluate", Path(__file__).with_name("evaluate-selection-results.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class EvaluationTest(unittest.TestCase):
    def test_abstention_is_not_counted_as_correct_or_as_open(self):
        labels = [dict(path="a",channel="eyes",expected="open",group="a",split="val"),
                  dict(path="b",channel="eyes",expected="closed",group="b",split="val")]
        predictions = {"a":{"legacy":{"eyes":"closed"},"candidate":{"eyes":"maybe"}},
                       "b":{"legacy":{"eyes":"open"},"candidate":{"eyes":"unknown"}}}
        result = module.evaluate(labels,predictions)
        self.assertEqual(result["legacy"]["eyes"]["false_positive"],1)
        self.assertEqual(result["legacy"]["eyes"]["false_negative"],1)
        self.assertEqual(result["candidate"]["eyes"]["abstention_rate"],1)
        self.assertIsNone(result["candidate"]["eyes"]["decided_accuracy"])

    def test_related_samples_cannot_cross_train_validation_splits(self):
        rows = [dict(path="a",channel="eyes",expected="open",group="burst1",split="train"),
                dict(path="b",channel="eyes",expected="open",group="burst1",split="val")]
        with self.assertRaisesRegex(ValueError,"multiple splits"):
            module.evaluate(rows,{})


if __name__ == "__main__":
    unittest.main()
