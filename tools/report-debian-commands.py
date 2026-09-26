"""Summarize the command audit without equating startup with functional coverage."""

import argparse
from collections import Counter
import csv
import json
from pathlib import Path
import re


def load(path):
    return json.loads(path.read_text(encoding="utf-8"))


def evidence(record):
    parts = [record.get("stderr", ""), record.get("stdout", "")]
    parts.extend(record.get("fixture_diagnostics", {}).values())
    return " | ".join(" ".join(part.split()) for part in parts if part.strip())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--results", type=Path, required=True)
    parser.add_argument("--csv", type=Path, required=True)
    args = parser.parse_args()
    inventory = load(args.results / "inventory.json")
    roots = [load(path)["root"] for path in args.results.glob("root-location*.json")]
    reports = {
        phase: load(args.results / f"{phase}.json") for phase in ("smoke", "functional")
    }
    by_phase = {
        phase: {r["path"]: r for r in report["results"]}
        for phase, report in reports.items()
    }
    paths = {entry["path"] for entry in inventory}
    for phase, rows in by_phase.items():
        if set(rows) != paths:
            raise ValueError(f"Incomplete {phase} report; run the full inventory first")
    if reports["smoke"]["images"] != reports["functional"]["images"]:
        raise ValueError("The phases tested different distribution images")

    rows = []
    symbols = {}
    for entry in inventory:
        path = entry["path"]
        row = dict(path=path, target=entry["target"], alias=entry["alias"])
        for phase in by_phase:
            record = by_phase[phase][path]
            detail = evidence(record)
            for root in roots:
                detail = detail.replace("\\\\?\\" + root, "<rootfs>").replace(
                    root, "<rootfs>"
                )
            row[phase + "_status"] = record["status"]
            row[phase + "_failure"] = record.get("failure", "")
            row[phase + "_exit_code"] = record.get("exit_code", "")
            row[phase + "_seconds"] = record.get("seconds", "")
            row[phase + "_purpose"] = record.get("purpose", record.get("reason", ""))
            row[phase + "_evidence"] = (
                detail[:900]
                if record["status"] in ("failed", "timeout", "harness_error")
                else ""
            )
            row[phase + "_log"] = record.get("log_directory", "")
            for symbol in re.findall(r"unresolved symbol (\S+)", detail):
                symbols.setdefault(symbol, set()).add(path)
        rows.append(row)

    args.csv.parent.mkdir(parents=True, exist_ok=True)
    with args.csv.open("w", encoding="utf-8-sig", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=list(rows[0]), lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)
    counts = {
        phase: dict(Counter(r["status"] for r in report["results"]))
        for phase, report in reports.items()
    }
    attempted = sum(
        any(by_phase[phase][path]["status"] != "not_tested" for phase in by_phase)
        for path in paths
    )
    functional_attempted = sum(
        r["status"] != "not_tested" for r in by_phase["functional"].values()
    )
    summary = dict(
        total_entries=len(inventory),
        file_entries=sum(not entry["alias"] for entry in inventory),
        alias_entries=sum(entry["alias"] for entry in inventory),
        distinct_targets=len({entry["target"] for entry in inventory}),
        attempted_entries=attempted,
        functional_attempted=functional_attempted,
        counts=counts,
        failure_categories={
            phase: dict(
                Counter(
                    r.get("failure", "unknown")
                    for r in report["results"]
                    if r["status"] in ("failed", "timeout", "harness_error")
                )
            )
            for phase, report in reports.items()
        },
        unresolved_symbols={
            symbol: sorted(affected) for symbol, affected in sorted(symbols.items())
        },
        images=reports["functional"]["images"],
        tested_distribution=reports["functional"]["distribution"],
        updated={phase: report["updated"] for phase, report in reports.items()},
        notes="Counts refer to command paths, including aliases; functional passes verify only their recorded scenario.",
    )
    (args.results / "summary.json").write_text(
        json.dumps(summary, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    print(
        json.dumps(
            {k: v for k, v in summary.items() if k != "unresolved_symbols"},
            ensure_ascii=False,
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
