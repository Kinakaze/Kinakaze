"""Export all locked software rows, including missing and unrun entries."""

import argparse
import csv
import hashlib
import json
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--lock", type=Path, default=Path("config/debian-software-top500.lock.json")
    )
    parser.add_argument("--results", type=Path, required=True)
    parser.add_argument("--csv", type=Path, required=True)
    args = parser.parse_args()
    lock_data = args.lock.read_bytes()
    lock = json.loads(lock_data)
    report = json.loads(args.results.read_text(encoding="utf-8"))
    if hashlib.sha256(lock_data).hexdigest() != report["fingerprint"]["lock"]:
        parser.error("results belong to another software lock")
    results = {row["package"]: row for row in report["results"]}
    columns = (
        "order",
        "popularity_rank",
        "package",
        "locked_version",
        "tested_version",
        "package_state",
        "status",
        "available_commands",
        "total_commands",
        "functional_passed",
        "functional_executed",
        "commands_without_scenarios",
        "startup_status",
        "first_failure",
        "logs",
    )
    args.csv.parent.mkdir(parents=True, exist_ok=True)
    with args.csv.open("w", encoding="utf-8-sig", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=columns)
        writer.writeheader()
        for package in lock["packages"]:
            row = results.get(package["package"], {})
            tests = row.get("tests", [])
            failed = [test for test in tests if test["status"] != "passed"]
            failure = failed[0] if failed else {}
            writer.writerow(
                dict(
                    order=package["order"],
                    popularity_rank=package["popularity_rank"],
                    package=package["package"],
                    locked_version=package["version"],
                    tested_version=row.get("tested_version"),
                    package_state=row.get("package_state"),
                    status=row.get("status", "not_run"),
                    available_commands=len(row.get("available_commands", [])),
                    total_commands=len(package["commands"]),
                    functional_passed=sum(test["status"] == "passed" for test in tests),
                    functional_executed=len(tests),
                    commands_without_scenarios=";".join(
                        row.get("commands_without_scenarios", [])
                    ),
                    startup_status=row.get("startup", {}).get("status", "not_run"),
                    first_failure=failure.get("path", "")
                    + ": "
                    + failure.get("failure", failure.get("status", ""))
                    if failed
                    else "",
                    logs=";".join(test.get("log_directory", "") for test in failed),
                )
            )
    print(f"Exported {len(lock['packages'])} software rows to {args.csv}")


if __name__ == "__main__":
    main()
