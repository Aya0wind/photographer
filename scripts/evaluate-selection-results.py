"""Compare exported selection results with labeled photography samples.

Labels CSV: path,channel,expected,group,split (channel eyes/blur).
Results JSONL: {path, legacy:{eyes,blur}, candidate:{eyes,blur}}.
No model confidence score is treated as accuracy. Unknown/maybe are abstentions.
"""
import argparse
import csv
import json
from pathlib import Path


def evaluate(labels, predictions):
    group_splits = {}
    keys = set()
    output = {name: {channel: dict(total=0, false_positive=0, false_negative=0,
        abstained=0, missing=0, decided=0, correct=0) for channel in ["eyes","blur"]}
        for name in ["legacy","candidate"]}
    for row in labels:
        channel = row["channel"]
        expected = row["expected"]
        positive, negative = ("closed","open") if channel == "eyes" else ("soft","sharp")
        if channel not in ["eyes","blur"] or expected not in [positive,negative]:
            raise ValueError("Labels must contain a definitive eyes/blur ground truth")
        key = (row["path"],channel)
        if key in keys:
            raise ValueError(f"Duplicate label: {key}")
        keys.add(key)
        if not row.get("group") or not row.get("split"):
            raise ValueError("Every sample needs a person/burst group and split")
        group = row["group"]
        if group in group_splits and group_splits[group] != row["split"]:
            raise ValueError(f"Group appears in multiple splits: {group}")
        group_splits[group] = row["split"]
        for name in output:
            metric = output[name][channel]
            metric["total"] += 1
            result = predictions.get(row["path"],{}).get(name,{}).get(channel)
            if result is None:
                metric["missing"] += 1
            if result not in [positive,negative]:
                metric["abstained"] += 1
                continue
            metric["decided"] += 1
            metric["correct"] += int(result == expected)
            metric["false_positive"] += int(result == positive and expected == negative)
            metric["false_negative"] += int(result == negative and expected == positive)
    for channels in output.values():
        for metric in channels.values():
            metric["abstention_rate"] = metric["abstained"]/metric["total"] if metric["total"] else None
            metric["decided_accuracy"] = metric["correct"]/metric["decided"] if metric["decided"] else None
    return output


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--labels",type=Path,required=True)
    parser.add_argument("--results",type=Path,required=True)
    args = parser.parse_args()
    with args.labels.open(encoding="utf-8-sig",newline="") as f:
        labels = list(csv.DictReader(f))
    results = {}
    for line in args.results.read_text(encoding="utf-8").splitlines():
        row = json.loads(line)
        if row["path"] in results:
            raise ValueError("Duplicate prediction path")
        results[row["path"]] = row
    print(json.dumps(evaluate(labels,results),indent=2))


if __name__ == "__main__":
    main()
