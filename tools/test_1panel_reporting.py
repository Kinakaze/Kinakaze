"""Host regressions for committed store inventory and honest coverage reports."""

import hashlib
import os
from pathlib import Path
import runpy
import subprocess
import tempfile
import unittest


matrix = runpy.run_path(str(Path(__file__).with_name("test-1panel-apps.py")))


class StoreInventoryTests(unittest.TestCase):
    def test_sparse_checkout_and_dirty_metadata_do_not_change_inventory(self):
        with tempfile.TemporaryDirectory() as temporary:
            store = Path(temporary)
            environment = dict(os.environ, GIT_AUTHOR_NAME="Fixture", GIT_AUTHOR_EMAIL="fixture@example.invalid",
                               GIT_COMMITTER_NAME="Fixture", GIT_COMMITTER_EMAIL="fixture@example.invalid")

            def git(*arguments, data=None):
                return subprocess.check_output(["git", "-C", str(store), *arguments],
                                               input=data, env=environment).decode().strip()

            git("init", "--quiet")
            git("config", "core.autocrlf", "false")
            metadata = b"name: fixture\n"
            for name in ("astrbot", "unregistered-app"):
                folder = store / "apps" / name
                folder.mkdir(parents=True)
                (folder / "data.yml").write_bytes(metadata)
            (store / "apps" / "README.md").write_text("not an application", encoding="utf-8")
            git("add", ".")
            tree = git("write-tree")
            commit = git("commit-tree", tree, data=b"test fixture\n")
            git("update-ref", "HEAD", commit)
            git("sparse-checkout", "set", "apps/astrbot")
            self.assertFalse((store / "apps/unregistered-app").exists())
            (store / "apps/astrbot/data.yml").write_text("dirty", encoding="utf-8")
            applications, evidence = matrix["store_inventory"](store)
            self.assertEqual(list(applications), ["astrbot", "unregistered-app"])
            self.assertIsNotNone(applications["astrbot"])
            self.assertIsNone(applications["unregistered-app"])
            self.assertEqual(evidence["commit"], commit)
            self.assertEqual(evidence["metadata_sha256"],
                             {name: hashlib.sha256(metadata).hexdigest() for name in applications})


class CoverageTests(unittest.TestCase):
    def report(self, selected, results):
        return dict(applications=["astrbot", "unknown"], selected=selected, results=results)

    def test_selected_pass_does_not_hide_untested_applications(self):
        report = self.report(["astrbot"], [dict(name="astrbot", round=0, status="passed")])
        matrix["update_coverage"](report, 1)
        self.assertEqual(report["coverage"], dict(total=2, passed=1, percent=50, unselected=1))
        self.assertFalse(report["complete"])

    def test_missing_scenario_stays_in_denominator(self):
        report = self.report(["astrbot", "unknown"], [dict(name="astrbot", round=0, status="passed"),
                                                     dict(name="unknown", round=0, status="missing_scenario")])
        matrix["update_coverage"](report, 1)
        self.assertEqual(report["counts"], dict(passed=1, missing_scenario=1))
        self.assertEqual(report["coverage"]["percent"], 50)
        self.assertFalse(report["complete"])

    def test_every_repeat_must_pass(self):
        report = self.report(["astrbot", "unknown"], [
            dict(name=name, round=iteration, status="passed")
            for name in ("astrbot", "unknown") for iteration in range(2)])
        matrix["update_coverage"](report, 2)
        self.assertTrue(report["complete"])
        report["results"][-1]["status"] = "failed"
        matrix["update_coverage"](report, 2)
        self.assertFalse(report["complete"])
        self.assertEqual(report["coverage"]["passed"], 1)
        report["results"].pop()
        matrix["update_coverage"](report, 2)
        self.assertEqual(report["coverage"]["passed"], 1)


if __name__ == "__main__":
    unittest.main()
