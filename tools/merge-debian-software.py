"""Aggregate a sweep and explicit case rechecks with the same root and runtime.

Every row keeps its source report and the report's SHA-256. Case revisions can
differ; runtime images, package state and executable inventory cannot differ.
This is an audit aggregate, not a resumable execution checkpoint.
"""

import argparse
from collections import Counter
import copy
import hashlib
import json
import os
from pathlib import Path


def merge(paths, output):
    reports = [json.loads(path.read_text(encoding="utf-8")) for path in paths]
    base = reports[0]
    required = ("lock", "images", "package_state", "harness", "command_harness")
    rows, origins = {}, []
    for path, report in zip(paths, reports):
        if (
            report["root"] != base["root"]
            or report["total"] != base["total"]
            or report["root_inventory_sha256"] != base["root_inventory_sha256"]
            or any(
                report["fingerprint"][key] != base["fingerprint"][key]
                for key in required
            )
        ):
            raise ValueError(
                "cannot merge different roots, runtime, package state or inventory"
            )
        origins.append(
            dict(
                path=str(path.resolve()),
                sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
                fingerprint=report["fingerprint"],
            )
        )
        for original in report["results"]:
            if original["status"] == "in_progress":
                raise ValueError("cannot merge a running case")
            row = copy.deepcopy(original)
            row["source_report"] = len(origins) - 1
            for case in [
                *row["tests"],
                *([row["startup"]] if "startup" in row else []),
            ]:
                if "log_directory" in case:
                    case["log_directory"] = os.path.relpath(
                        path.parent / case["log_directory"], output.parent
                    )
            rows[row["package"]] = row
    result = copy.deepcopy(base)
    result["scope"] = __doc__
    result["source_reports"] = origins
    result["fingerprint"]["cases"] = "aggregate: see source_reports"
    result["results"] = sorted(rows.values(), key=lambda row: row["order"])
    counts = Counter(row["status"] for row in result["results"])
    counts["not_run"] = result["total"] - len(result["results"])
    result["counts"] = dict(counts)
    result["startup_counts"] = dict(
        Counter(
            row["startup"]["status"] for row in result["results"] if "startup" in row
        )
    )
    result["complete"] = counts["passed"] == result["total"]
    result["scan_complete"] = counts["not_run"] == 0
    result["functional_passed_percent"] = 100 * counts["passed"] / result["total"]
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(
        json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--results", type=Path, action="append", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.resolve() in {path.resolve() for path in args.results}:
        parser.error("aggregate must not overwrite a source report")
    try:
        report = merge(args.results, args.output)
    except ValueError as error:
        parser.error(str(error))
    print(json.dumps(report["counts"]))


if __name__ == "__main__":
    main()
