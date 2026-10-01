"""Ranking must include architecture-all tools and preserve untested rows."""

from contextlib import redirect_stdout
import csv
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import runpy
import tempfile
import unittest
from unittest.mock import patch


TOOLS = Path(__file__).resolve().parents[1]
LOCK = runpy.run_path(str(TOOLS / "lock-debian-software.py"))
REPORT = runpy.run_path(str(TOOLS / "report-debian-software.py"))
MERGE = runpy.run_path(str(TOOLS / "merge-debian-software.py"))


class SoftwareQueueTests(unittest.TestCase):
    def test_contents_includes_aliases_and_multiple_owners_but_no_library_files(self):
        content = gzip.compress(
            b"usr/bin/tool admin/alpha,utils/beta\n"
            b"bin/alias admin/alpha\n"
            b"usr/lib/library.so libs/dependency\n"
            b"usr/bin/sub/file admin/nested\n"
            b"usr/bin/X11 x11/x11-common\n"
            b"usr/bin/mh mail/mh-directory\n"
        )
        self.assertEqual(
            LOCK["command_owners"](content),
            {"alpha": {"/usr/bin/tool", "/bin/alias"}, "beta": {"/usr/bin/tool"}},
        )

    def test_architecture_all_and_unique_packages_follow_popcon_order(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            lists = root / "lists"
            lists.mkdir()
            index = "\n\n".join(
                f"Package: {name}\nVersion: 1\nArchitecture: {architecture}\n"
                f"Filename: pool/{name}.deb\nSize: 1\nSHA256: {'0' * 64}"
                for name, architecture in [
                    ("alpha", "amd64"),
                    ("beta", "all"),
                    ("library", "amd64"),
                ]
            )
            (lists / "fixture_Packages").write_text(index, encoding="utf-8")
            (root / "votes").write_text(
                "# header\n1 library 100 90 0 0 0\n"
                "2 beta 100 80 0 0 0\n3 beta 100 80 0 0 0\n"
                "4 alpha 100 70 0 0 0\n",
                encoding="utf-8",
            )
            (root / "Contents-amd64.gz").write_bytes(
                gzip.compress(b"usr/bin/alpha utils/alpha\n")
            )
            (root / "Contents-all.gz").write_bytes(
                gzip.compress(b"usr/bin/beta utils/beta\n")
            )
            argv = [
                "lock",
                "--popcon",
                str(root / "votes"),
                "--contents",
                str(root / "Contents-amd64.gz"),
                "--contents",
                str(root / "Contents-all.gz"),
                "--apt-lists",
                str(lists),
                "--output",
                str(root / "lock.json"),
                "--count",
                "2",
            ]
            with patch("sys.argv", argv), redirect_stdout(io.StringIO()):
                LOCK["main"]()
            lock = json.loads((root / "lock.json").read_text(encoding="utf-8"))
            self.assertEqual(
                [p["package"] for p in lock["packages"]], ["beta", "alpha"]
            )
            self.assertEqual([p["order"] for p in lock["packages"]], [1, 2])
            self.assertEqual([p["popularity_rank"] for p in lock["packages"]], [2, 4])
            self.assertEqual(len(lock["sources"]["contents"]), 2)

    def test_csv_retains_unrun_software_in_the_fixed_denominator(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            lock = {
                "packages": [
                    dict(
                        order=i,
                        popularity_rank=i,
                        package=name,
                        version="1",
                        commands=["/usr/bin/" + name],
                    )
                    for i, name in enumerate(("tested", "unrun"), 1)
                ]
            }
            lock_data = json.dumps(lock).encode()
            (root / "lock.json").write_bytes(lock_data)
            report = {
                "fingerprint": {"lock": hashlib.sha256(lock_data).hexdigest()},
                "results": [
                    dict(
                        package="tested", status="passed", tests=[{"status": "passed"}]
                    )
                ],
            }
            (root / "results.json").write_text(json.dumps(report), encoding="utf-8")
            with (
                patch(
                    "sys.argv",
                    [
                        "report",
                        "--lock",
                        str(root / "lock.json"),
                        "--results",
                        str(root / "results.json"),
                        "--csv",
                        str(root / "result.csv"),
                    ],
                ),
                redirect_stdout(io.StringIO()),
            ):
                REPORT["main"]()
            with (root / "result.csv").open(encoding="utf-8-sig", newline="") as stream:
                rows = list(csv.DictReader(stream))
            self.assertEqual(len(rows), 2)
            self.assertEqual(rows[1]["status"], "not_run")
            self.assertEqual(rows[1]["functional_passed"], "0")

    @unittest.skipUnless(os.name == "nt", "NTFS directory semantics")
    def test_clone_retains_distinct_case_variants(self):
        clone = runpy.run_path(str(TOOLS / "clone-debian-root.py"))["main"]
        artifacts = TOOLS.parent / "artifacts"
        artifacts.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=artifacts) as directory:
            task = Path(directory)
            empty, source, target = task / "empty", task / "source", task / "target"
            empty.mkdir()
            for origin, destination in ((empty, source), (source, target)):
                with patch(
                    "sys.argv",
                    ["clone", "--source", str(origin), "--root", str(destination)],
                ):
                    clone()
                if destination == source:
                    (source / "head").write_text("lower", encoding="utf-8")
                    (source / "HEAD").write_text("upper", encoding="utf-8")
                    (source / "Pod").mkdir()
                    (source / "pod").mkdir()
                    (source / "Pod" / "file").write_text("module", encoding="utf-8")
                    (source / "pod" / "file").write_text("manual", encoding="utf-8")
            self.assertEqual((target / "head").read_text(encoding="utf-8"), "lower")
            self.assertEqual((target / "HEAD").read_text(encoding="utf-8"), "upper")
            self.assertEqual(
                (target / "Pod" / "file").read_text(encoding="utf-8"), "module"
            )
            self.assertEqual(
                (target / "pod" / "file").read_text(encoding="utf-8"), "manual"
            )

    def test_rechecks_retain_provenance_and_reject_changed_runtime(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            base = dict(
                root="same-root",
                total=2,
                root_inventory_sha256="same-images",
                fingerprint=dict(
                    lock="same",
                    images={"libc": "v1"},
                    package_state="same",
                    harness="same",
                    command_harness="same",
                ),
                results=[
                    dict(order=1, package="first", status="passed", tests=[]),
                    dict(order=2, package="second", status="failed", tests=[]),
                ],
            )
            update = json.loads(json.dumps(base))
            update["results"] = [
                dict(order=2, package="second", status="passed", tests=[])
            ]
            for filename, value in (("base.json", base), ("update.json", update)):
                (root / filename).write_text(json.dumps(value), encoding="utf-8")
            paths = [root / "base.json", root / "update.json"]
            result = MERGE["merge"](paths, root / "aggregate.json")
            self.assertEqual(result["counts"], {"passed": 2, "not_run": 0})
            self.assertEqual(result["results"][1]["source_report"], 1)
            self.assertEqual(
                result["source_reports"][0]["sha256"],
                hashlib.sha256(paths[0].read_bytes()).hexdigest(),
            )
            update["fingerprint"]["images"]["libc"] = "v2"
            paths[1].write_text(json.dumps(update), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "different roots, runtime"):
                MERGE["merge"](paths, root / "invalid.json")


if __name__ == "__main__":
    unittest.main()
