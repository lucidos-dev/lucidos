"""Hermetic CLI fixtures for the crate tier, size, and graph gates."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]


class Fixture(unittest.TestCase):
    def setUp(self):
        (SCRIPTS.parent / "target").mkdir(exist_ok=True)
        directory = tempfile.TemporaryDirectory(dir=SCRIPTS.parent / "target")
        self.addCleanup(directory.cleanup)
        self.repo = Path(directory.name)
        env = {key: value for key, value in os.environ.items()
               if not key.startswith("GIT_")}
        env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
                   PATH=f"{self.repo}:{env['PATH']}")
        self.env = env
        self.git("init", "-q", "-b", "main")
        self.write("cargo", "#!/bin/sh\ncat metadata.json\n").chmod(0o755)
        self.packages = []
        self.write("crates/crate-tiers.tsv", "")

    def write(self, path, text):
        full = self.repo / path
        full.parent.mkdir(parents=True, exist_ok=True)
        full.write_text(text)
        return full

    def git(self, *args):
        subprocess.run(["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                        *args], cwd=self.repo, env=self.env, check=True, capture_output=True)

    def package(self, name, *edges, kinds=("lib",)):
        crate = f"crates/{name}"
        self.packages.append({
            "id": name,
            "name": name,
            "manifest_path": str(self.repo / crate / "Cargo.toml"),
            "dependencies": [{"name": dep, "kind": kind,
                              "path": str(self.repo / "crates" / dep)} for dep, kind in edges],
            "targets": [{"name": target, "kind": [target],
                         "src_path": str(self.repo / crate / path)}
                        for target, path in [("lib", "src/lib.rs"), ("bin", "src/bin/example.rs"),
                                             ("test", "tests/feature.rs")]
                        if target in kinds],
        })

    def run_check(self, name, *args, failure=None):
        self.write("metadata.json", json.dumps({
            "packages": self.packages,
            "workspace_members": [item["id"] for item in self.packages],
        }))
        result = subprocess.run([SCRIPTS / f"check-crate-{name}.sh", *args], cwd=self.repo,
                                env=self.env, capture_output=True, text=True)
        if failure:
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn(failure, result.stderr)
        else:
            self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout


class TierTests(Fixture):
    def test_dependency_boundaries(self):
        self.package("lucidos-low", ("lucidos-sibling", "dev"),
                     ("lucidos-leaf", None), ("lucidos-higher", "build"))
        self.package("lucidos-sibling")
        self.package("lucidos-leaf")
        self.package("lucidos-higher")
        self.package("lucidos-gateway", ("lucidos-core", None))
        self.package("lucidos-core")
        self.package("lucidos-engine-bin", ("lucidos-leaf", None))
        self.write("crates/crate-tiers.tsv", """lucidos-low	0	engine
lucidos-sibling	0	engine
lucidos-leaf	7	leaf
lucidos-higher	1	engine
lucidos-gateway	4
lucidos-core	2	engine
lucidos-engine-bin	8	engine
""")
        for reason in ("lucidos-sibling (dev): tier 0", "lucidos-higher (build): tier 0",
                       "only lucidos-engine-bin may depend on a leaf",
                       "lucidos-gateway -> lucidos-core (normal): non-engine"):
            self.run_check("tiers", failure=reason)
        self.packages[0]["dependencies"].clear()
        self.packages[4]["dependencies"].clear()
        self.run_check("tiers")
        self.package("lucidos-unclassified")
        self.run_check("tiers", failure="lucidos-unclassified: missing tier")


class SizeTests(Fixture):
    def test_standalone_test_counts_shared_modules(self):
        self.package("lucidos-example", kinds=("test",))
        path = "crates/lucidos-example/tests"
        self.write("crates/crate-size-ceilings.tsv", "")
        self.write(f"{path}/feature.rs", '#[path = "support/start.rs"]\nmod support;\n')
        self.write(f"{path}/support/start.rs", "mod nested;\n")
        self.write(f"{path}/support/nested.rs", "\n" * 50_000)
        self.git("add", "crates")
        self.git("commit", "-qm", "baseline")
        self.run_check("size", failure="test (test): 50003 lines exceeds 50000")

    def test_targets_and_ratchet(self):
        self.package("lucidos-example", kinds=("lib", "bin"))
        path = "crates/lucidos-example/src"
        ceiling = "crates/crate-size-ceilings.tsv"
        self.write(ceiling, "lucidos-example\t52000\n")
        self.write(f"{path}/lib.rs", "\n" * 51_000)
        binary = self.write(f"{path}/bin/example.rs", "\n" * 50_000)
        self.git("add", "crates")
        self.git("commit", "-qm", "baseline")
        self.run_check("size")
        self.run_check("size", "--reset", "lucidos-example")
        self.assertIn("lucidos-example\t56100", (self.repo / ceiling).read_text())
        self.run_check("size")
        self.write(ceiling, "lucidos-example\t56101\n")
        self.run_check("size", failure="ceiling rose from 52000 to 56101, past 56100")
        self.write(ceiling, "lucidos-example\t52000\n")
        binary.write_text("\n" * 52_001)
        self.run_check("size", failure="bin (bin): 52001 lines exceeds 52000")
        self.run_check("size", "--reset", "lucidos-example")
        self.assertIn("lucidos-example\t57202", (self.repo / ceiling).read_text())
        self.run_check("size")
        binary.write_text("\n" * 50_000)
        self.write(f"{path}/lib.rs", "\n" * 49_999 + "last line")
        self.run_check("size", "--reset", "lucidos-example")
        self.run_check("size")
        self.write(f"{path}/lib.rs", "\n" * 50_000 + "last line")
        self.run_check("size", failure="lib (lib): 50001 lines exceeds 50000")


class GraphTests(Fixture):
    def test_snapshot_counts_diamond_and_test_targets(self):
        base, left, right, top = "base", "lucidos-left", "lucidos-right", "lucidos-top"
        self.package(base)
        self.package(left, (base, "dev"))
        self.package(right, (base, "build"))
        self.package(top, (left, None), (right, None), kinds=("lib", "test"))
        self.packages[0]["dependencies"].append({"name": top, "kind": None, "path": None})
        self.write("crates/crate-tiers.tsv",
                   f"{base}\t0\n{left}\t1\n{right}\t1\n{top}\t2\n")
        for name, lines in [(base, 1), (left, 2), (right, 3), (top, 4)]:
            self.write(f"crates/{name}/src/lib.rs", "\n" * lines)
        self.write(f"crates/{top}/tests/feature.rs", "\n" * 5)
        self.git("add", "crates")
        self.git("commit", "-qm", "baseline")
        graph = json.loads(self.run_check("graph", "--json"))
        self.assertEqual(graph["crates"][base]["blast_radius"], 14)
        self.assertEqual(graph["crates"][base]["dependents"], [left, right, top])
        self.assertEqual(graph["crates"][top]["target_lines"], 9)
        self.assertEqual(graph["crates"][top]["change_commits"], 1)
        self.write(f"crates/{top}/tests/feature.rs", "next edit\n")
        self.git("add", "crates")
        self.git("commit", "-qm", "test edit")
        counts = json.loads(self.run_check("graph", "--json"))["crates"]
        self.assertEqual(counts[top]["change_commits"], 2)
        self.assertEqual(counts[base]["change_commits"], 1)
        self.assertEqual(graph["depth"], 2)
        self.assertEqual(graph["longest_path"], [top, left, base])
        self.assertIn({"from": left, "to": base, "kind": "dev"}, graph["edges"])
        self.assertEqual(len(graph["revision"]), 40)
        self.packages[-1]["dependencies"].pop()
        dev_only = json.loads(self.run_check("graph", "--json"))
        self.assertEqual(dev_only["crates"][base]["dependents"], [left, right])
        self.assertEqual(dev_only["depth"], 2)

    def test_dev_dependent_of_indirect_normal_dependent(self):
        base, mid, top, tests = "lucidos-base", "lucidos-mid", "lucidos-top", "lucidos-tests"
        self.package(base)
        self.package(mid, (base, None))
        self.package(top, (mid, None))
        self.package(tests, (top, "dev"), kinds=("test",))
        self.write("crates/crate-tiers.tsv", f"{base}\t0\n{mid}\t1\n{top}\t2\n{tests}\t3\n")
        self.write(f"crates/{mid}/src/lib.rs", "\n" * 2)
        self.write(f"crates/{top}/src/lib.rs", "\n" * 3)
        self.write(f"crates/{tests}/tests/feature.rs", "\n" * 5)
        self.git("add", "crates")
        self.git("commit", "-qm", "baseline")

        graph = json.loads(self.run_check("graph", "--json"))
        self.assertEqual(graph["crates"][base]["dependents"], [mid, tests, top])
        self.assertEqual(graph["crates"][base]["blast_radius"], 10)

    def test_dependents_ratchet(self):
        base, top, extra, leaf = "lucidos-base", "lucidos-top", "lucidos-extra", "lucidos-leaf"
        self.package(base)
        self.package(top, (base, "dev"), kinds=("test",))
        self.package(extra)
        self.write("crates/crate-tiers.tsv", f"{base}\t0\n{top}\t1\n{extra}\t1\n{leaf}\t1\n")
        source = self.write(f"crates/{top}/tests/feature.rs", "one\ntwo\n")
        baseline = "crates/crate-graph-baseline.tsv"
        self.write(baseline, f"depth\t1\n{base}\t{top}\n{extra}\t\n{top}\t\n")
        self.git("add", "crates")
        self.git("commit", "-qm", "baseline")
        self.run_check("graph")
        edge = {"name": base, "kind": None, "path": str(self.repo / "crates" / base)}

        source.write_text("one\ntwo\nthree\n")
        self.run_check("graph")
        self.packages[2]["dependencies"] = [edge]
        self.run_check("graph", failure=f"{base}: gained dependents {extra}")
        self.write(baseline, self.run_check("graph", "--baseline"))
        self.run_check("graph", failure=f"{base}: gained dependents {extra}")
        self.packages[1]["dependencies"] = []
        self.write(baseline, self.run_check("graph", "--baseline"))
        self.run_check("graph", failure=f"{base}: gained dependents {extra}")

        self.packages[1]["dependencies"] = [dict(edge, kind="dev")]
        self.packages[2]["dependencies"] = []
        self.package(leaf, (base, None))
        self.run_check("graph", failure=f"{base}: dependents differ from the baseline")
        self.write(baseline, self.run_check("graph", "--baseline"))
        self.run_check("graph")
        self.write(baseline, "depth\t1\nlucidos-removed\t\n")
        self.run_check("graph", failure=f"{base}: missing dependents baseline")
        self.run_check("graph", failure="lucidos-removed: stale dependents baseline")

    def test_crate_renamed_in_place_is_not_extracted(self):
        base, old, new = "lucidos-base", "lucidos-old", "lucidos-new"
        self.package(base)
        self.package(old)
        self.write("crates/crate-tiers.tsv", f"{base}\t0\n{old}\t1\n{new}\t1\n")
        self.write(f"crates/{old}/Cargo.toml", "")
        baseline = "crates/crate-graph-baseline.tsv"
        self.write(baseline, f"depth\t0\n{base}\t\n{old}\t\n")
        self.git("add", "crates")
        self.git("commit", "-qm", "baseline")
        self.run_check("graph")

        self.packages[1].update(id=new, name=new, dependencies=[
            {"name": base, "kind": None, "path": str(self.repo / "crates" / base)}])
        self.write(baseline, self.run_check("graph", "--baseline"))
        self.run_check("graph", failure=f"{base}: gained dependents {new}")

    def test_new_crate_cap_depth_and_cycle(self):
        base, mid, top = "lucidos-base", "lucidos-mid", "lucidos-top"
        self.package(top, kinds=("test",))
        self.write("crates/crate-tiers.tsv", f"{top}\t2\n")
        path = "crates/crate-graph-baseline.tsv"
        self.write(path, f"depth\t0\n{top}\t\n")
        source = self.write(f"crates/{top}/tests/feature.rs", "\n" * 150_001)
        self.git("add", "crates")
        self.git("commit", "-qm", "baseline")
        self.package(base)
        self.package(mid, (base, "build"))
        self.packages[0]["dependencies"] = self.packages[-1]["dependencies"].copy()
        self.write("crates/crate-tiers.tsv", f"{base}\t0\n{mid}\t1\n{top}\t2\n")
        self.write(path, self.run_check("graph", "--baseline"))
        self.run_check("graph", failure=f"{base}: new crate exceeds 150000")
        self.run_check("graph", failure="dependency depth rose from 0 to 1")
        source.write_text("\n" * 150_000)
        self.write(path, self.run_check("graph", "--baseline"))
        self.git("add", "crates")
        self.git("commit", "-qm", "depth-one baseline")
        self.run_check("graph")
        self.packages[0]["dependencies"] = [
            {"name": mid, "kind": None, "path": str(self.repo / "crates" / mid)}
        ]
        self.write(path, self.run_check("graph", "--baseline"))
        self.run_check("graph", failure="dependency depth rose from 1 to 2")
        self.packages[1]["dependencies"] = [
            {"name": top, "kind": "dev", "path": str(self.repo / "crates" / top)}
        ]
        self.run_check("graph", failure="workspace dependency cycle")


if __name__ == "__main__":
    unittest.main()
