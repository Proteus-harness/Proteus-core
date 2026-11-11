#!/usr/bin/env python3
"""Prepare six pinned public coding cases and grade in a disposable namespace."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import resource

ROOT = Path(__file__).resolve().parents[3]
DATA = ROOT / "target/performance/coding-harness"
PINS = {"polyglot": "7e0611e77b54e2dea774cdc0aa00cf9f7ed6144f",
        "tb2": "2fd12b88aafdd04a52c298e3940bcb189f9766d6"}
POLYGLOT = ("forth", "poker", "dominoes", "variable-length-quantity")
TERMINAL = ("cancel-async-tasks", "log-summary-date-ranges")
PYTHON = DATA / "grader-venv/bin/python"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def manifest(directory):
    return {str(path.relative_to(directory)): digest(path)
            for path in sorted(directory.rglob("*")) if path.is_file()}


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")


def sandbox(workspace, private, command, timeout=120):
    """Only disposable work, immutable grading inputs and interpreter are visible."""
    runtime = PYTHON.resolve().parents[2]
    args = ["bwrap", "--die-with-parent", "--new-session", "--unshare-all",
            "--ro-bind", "/usr", "/usr", "--ro-bind", "/etc", "/etc",
            "--symlink", "usr/bin", "/bin", "--symlink", "usr/lib", "/lib",
            "--symlink", "usr/lib64", "/lib64", "--proc", "/proc", "--dev", "/dev",
            "--tmpfs", "/tmp", "--dir", "/home", "--dir", "/logs",
            "--bind", str(workspace), "/app", "--ro-bind", str(private), "/grader",
            "--ro-bind", str(PYTHON.parent.parent), "/venv",
            "--ro-bind", str(runtime), str(runtime), "--chdir", "/app",
            "--clearenv", "--setenv", "PATH", "/venv/bin:/usr/bin:/bin",
            "--setenv", "HOME", "/tmp", "--setenv", "PYTHONDONTWRITEBYTECODE", "1",
            *command]
    with tempfile.TemporaryFile() as output:
        def limits():
            resource.setrlimit(resource.RLIMIT_FSIZE, (8 * 1024 * 1024, 8 * 1024 * 1024))
            resource.setrlimit(resource.RLIMIT_CPU, (90, 90))
            resource.setrlimit(resource.RLIMIT_AS, (2 * 1024 ** 3, 2 * 1024 ** 3))
        process = subprocess.Popen(args, stdout=output, stderr=subprocess.STDOUT,
                                   start_new_session=True, preexec_fn=limits)
        timed_out = False
        try:
            code = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            os.killpg(process.pid, signal.SIGKILL)
            code = process.wait()
        output.seek(0)
        text = output.read(128_000).decode(errors="replace")
    return {"returncode": code, "timed_out": timed_out, "output": text}


def prepare(output):
    for name, pin in PINS.items():
        checkout = DATA / name
        actual = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=checkout, text=True).strip()
        if actual != pin:
            raise ValueError(f"{name}: expected {pin}, found {actual}")
        if subprocess.check_output(["git", "status", "--porcelain"], cwd=checkout, text=True):
            raise ValueError(f"{name}: source checkout is dirty")
    output.mkdir(parents=True, exist_ok=False)
    for name in POLYGLOT + TERMINAL:
        case = output / name
        repo, private = case / "repo", case / "grader"
        repo.mkdir(parents=True)
        private.mkdir()
        if name in POLYGLOT:
            family = "polyglot"
            source = DATA / family / "python/exercises/practice" / name
            for path in source.glob("*.py"):
                shutil.copy2(path, repo / path.name)
                if path.name.endswith("_test.py"):
                    shutil.copy2(path, private / path.name)
                else:
                    module = path.name
            for path in sorted((source / ".docs").glob("instructions*.md")):
                shutil.copy2(path, repo / path.name)
            shutil.copy2(source / ".meta/example.py", private / "reference.py")
            command = ["/venv/bin/python", "-m", "pytest", "-q", "-o", "python_files=*_test.py", "/grader"]
            # Authoritative tests import the disposable submission from /app.
            (private / "conftest.py").write_text("import sys\nsys.path.insert(0, '/app')\n")
            prompt = (f"Implement the {name} exercise in {module}. Read instructions.md"
                      " and the supplied tests, preserve the public API, and run the tests."
                      " Modify the implementation, keeping the supplied tests unchanged."
                      " Additional requirements are in instructions.append.md when present.")
            reference = {"kind": "python", "module": module}
        else:
            family = "tb2"
            source = DATA / family / name
            for path in (source / "tests").glob("*.py"):
                shutil.copy2(path, private / path.name)
            shutil.copy2(source / "solution/solve.sh", private / "reference.sh")
            prompt = (source / "instruction.md").read_text().replace("/app/", "./")
            prompt += "\nThe task workspace is the current directory. Use Python 3 standard library tools available in this environment.\n"
            command = ["/venv/bin/python", "-m", "pytest", "-q", "/grader/test_outputs.py"]
            reference = {"kind": "shell"}
            if name == "cancel-async-tasks":
                (repo / "run.py").write_text("async def run_tasks(tasks, max_concurrent):\n    raise NotImplementedError\n")
            else:
                shutil.copy2(source / "environment/log_generator_deterministic.py", private / "generate.py")
                result = sandbox(repo, private, ["/venv/bin/python", "/grader/generate.py"])
                if result["returncode"]:
                    raise RuntimeError(result)
        (case / "prompt.txt").write_text(prompt + "\n")
        info = {"id": name, "family": family, "source_rev": PINS[family],
                "source_url": ("https://github.com/Aider-AI/polyglot-benchmark" if family == "polyglot"
                               else "https://github.com/harbor-framework/terminal-bench-2"),
                "source_hashes": manifest(source), "repo_hashes": manifest(repo),
                "grader_hashes": manifest(private), "prompt_sha256": digest(case / "prompt.txt"),
                "command": command, "reference": reference,
                "environment": {"python": "3.13.9", "pytest": "8.4.1", "network": False},
                "adaptations": (["/app paths in instructions become workspace-relative; original grader still sees /app",
                                  "stdlib Python 3.13.9 in bwrap replaces Docker Python 3.13 image",
                                  "grader installed pytest only; no download during grading"] if family == "tb2" else
                                 ["public tests visible; immutable authoritative copies graded externally"])}
        write_json(case / "case.json", info)
        print(case, flush=True)


def grade(case, workspace, output, reference=False):
    info = json.loads((case / "case.json").read_text())
    if manifest(case / "grader") != info["grader_hashes"]:
        raise ValueError("authoritative grader changed")
    unchanged = all((workspace / name).resolve().is_relative_to(workspace)
                    and not (workspace / name).is_symlink()
                    and (workspace / name).is_file() and digest(workspace / name) == sha
                    for name, sha in info["repo_hashes"].items()
                    if name.endswith("_test.py") or name.startswith("logs/"))
    with tempfile.TemporaryDirectory(prefix="proteus-grade-") as temporary:
        work = Path(temporary) / "submission"
        shutil.copytree(workspace, work, symlinks=True,
                        ignore=shutil.ignore_patterns("__pycache__", ".pytest_cache", ".git"))
        private = case / "grader"
        if reference:
            if info["reference"]["kind"] == "python":
                shutil.copy2(private / "reference.py", work / info["reference"]["module"])
            else:
                script = (private / "reference.sh").read_text()
                # Packages from the upstream solution are already system tools.
                script = "\n".join(line for line in script.splitlines() if not line.startswith("apt-get "))
                (work / "reference.sh").write_text(script)
                result = sandbox(work, private, ["bash", "reference.sh"])
                if result["returncode"]:
                    raise RuntimeError(result)
        helper = private / "test.py"
        if helper.exists():
            (work / "test.py").unlink(missing_ok=True)
            shutil.copy2(helper, work / "test.py")
        grading_inputs = Path(temporary) / "grader"
        grading_inputs.mkdir()
        for path in private.glob("*.py"):
            if path.name.startswith("test") or path.name.endswith("_test.py") or path.name == "conftest.py":
                shutil.copy2(path, grading_inputs / path.name)
        result = sandbox(work, grading_inputs, info["command"])
    result.update({"case": info["id"], "fixture_unchanged": unchanged,
                   "passed": result["returncode"] == 0 and unchanged,
                   "reference_control": reference})
    if output:
        write_json(output, result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    prep = sub.add_parser("prepare")
    prep.add_argument("--output", type=Path, default=DATA / "cases")
    check = sub.add_parser("grade")
    check.add_argument("case", type=Path)
    check.add_argument("--workspace", type=Path, required=True)
    check.add_argument("--output", type=Path)
    check.add_argument("--reference", action="store_true")
    args = parser.parse_args()
    if args.action == "prepare":
        prepare(args.output.resolve())
        return 0
    result = grade(args.case.resolve(), args.workspace.resolve(), args.output, args.reference)
    print(json.dumps(result, indent=2))
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
