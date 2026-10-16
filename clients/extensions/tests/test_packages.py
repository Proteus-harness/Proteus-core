"""Behavioral checks for moved features and standalone delivery."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "sdk"))
from proteus_extension import Extension


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, ROOT / path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


report = load("usage_report", "packages/usage/report.py")
tree = load("file_tree", "packages/files/tree.py")
analysis = load("turn_analysis", "packages/analysis/main.py")
plan = load("plan", "packages/plan/main.py")


class PackagesTests(unittest.TestCase):
    def test_optional_usage_and_cache_cost(self):
        request = {"model":{"provider":"fixture", "model":"gpt-6-astra"}, "origin":"direct", "status":"completed",
                   "usage":{"input_tokens":10000, "output_tokens":1000, "cached_input_tokens":2000, "reasoning_output_tokens":400}}
        self.assertAlmostEqual(report.cost(request), .132)
        total = report.totals([request])
        self.assertEqual(total["reasoning_output_tokens"],400)
        self.assertIsNone(total["cache_creation_input_tokens"])
        request["usage"]["cached_input_tokens"] = 10001
        self.assertIsNone(report.cost(request))
        request["usage"] = None
        total = report.totals([request])
        self.assertEqual((total["reported"], total["priced"]), (0,0))
        self.assertIsNone(total["reasoning_output_tokens"])

    def test_context_keeps_small_nonzero_categories(self):
        cells = report.cells({"history":{"estimated_tokens":100000}, "latest_usage":{
            "max_input_tokens":200000, "compaction_trigger_tokens":180000,
            "categories":[{"name":"large","tokens":99999},{"name":"tiny","tokens":1}]}})
        self.assertEqual(len(cells),200)
        for label in ("tiny:","Резерв:","Свободно:"):
            self.assertTrue(any(cell["label"].startswith(label) for cell in cells))

    def test_search_retains_ancestors_and_directory_order(self):
        result={"entries":[{"path":"src/z.rs","kind":"file"},{"path":"src/a/b.py","kind":"file"},{"path":"Cargo.lock","kind":"file"}]}
        rows=tree.search_rows(result,lambda path,label,depth,directory:dict(id=path,depth=depth,expanded=False if directory else None))
        self.assertEqual([(row["id"],row["depth"]) for row in rows],[("src",0),("src/a",1),("src/a/b.py",2),("src/z.rs",1),("Cargo.lock",0)])
        self.assertTrue(rows[0]["expanded"])
        self.assertEqual(tree.language("Cargo.lock"),"toml")
        self.assertEqual(tree.language("src/X.HPP"),"cpp")

    def test_hook_failures_follow_public_contract(self):
        step={"data":{"kind":"hook","trace":{"input":{"event":{"event":"before_tool"}},"steps":[{"outcome":{"status":"failed","message":"fixture","model_failure":None}}]}}}
        self.assertEqual(analysis.title(step),"before_tool")
        self.assertEqual(analysis.status(step),"Ошибка")

    def test_plan_follows_incremental_updates_without_text(self):
        worker=plan.Plan()
        messages=[]
        worker.send=messages.append
        worker.handle({"type":"session_snapshot","session_dir":"/one","state":{},"entries":[]})
        worker.handle({"type":"render","view":"badge","surface":"badge","session_dir":"/one"})
        entry={"role":"assistant","tool":{"name":"update_plan","args":{"plan":[{"step":"one","status":"pending"}]},"effective_args":None}}
        worker.handle({"type":"session_update","session_dir":"/one","update":{"type":"entries","start":0,"removed":0,"entries":[entry]}})
        self.assertEqual(messages[-1]["face"]["text"],"0/1")
        worker.handle({"type":"session_update","session_dir":"/other","update":{"type":"entries","start":0,"removed":1,"entries":[]}})
        self.assertEqual(len(worker.entries),1)
        worker.handle({"type":"event","event":"session_opened","session_dir":"/two"})
        self.assertNotIn("0/1",json.dumps(messages[-1]))

    def test_session_command_acknowledges_errors_and_changes(self):
        worker=Extension();messages=[];worker.send=messages.append
        worker.session="/one";worker.state={"config":{"profile":"before"}}
        worker.set_session("effort","high")
        identity=messages[-1]["id"]
        worker.handle({"type":"command_result","id":identity,"session_dir":"/one","config":None,"error":"fixture failure"})
        self.assertFalse(worker.pending)
        self.assertEqual(worker.command_error,"fixture failure")
        worker.set_session("mode","plan")
        worker.handle({"type":"command_result","id":messages[-1]["id"],"session_dir":"/one","config":{"permission_mode":"plan"},"error":None})
        self.assertEqual(worker.state["config"]["permission_mode"],"plan")

    def test_every_standalone_package_runs_without_checkout_imports(self):
        with tempfile.TemporaryDirectory() as directory:
            subprocess.run([sys.executable,str(ROOT/"package.py"),"--bundle",directory],check=True)
            result=subprocess.run([sys.executable,str(ROOT/"tests/wire.py"),"--packages",directory],capture_output=True,text=True,timeout=45)
            self.assertEqual(result.returncode,0,result.stderr)
            self.assertIn("12 packages:",result.stdout)
            for package in Path(directory).iterdir():
                self.assertEqual(json.loads((package/"extension.json").read_text())["author"],"Proteus Project")


if __name__ == "__main__": unittest.main()
