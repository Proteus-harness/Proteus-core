"""Prepare and control the extended DeepSeek coding corpus."""
import argparse
import json
from pathlib import Path
import shutil
import tempfile

import corpus

HERE = Path(__file__).resolve().parent
DATA = corpus.DATA
TB2 = ("write-compressor", "schemelike-metacircular-eval", "db-wal-recovery")


def prepare(case, path):
    repo = path / "repo"
    private = path / "grader"
    repo.mkdir(parents=True)
    private.mkdir()
    followups = []
    if case in TB2:
        import long_upstream
        prompt, command, reference, info = long_upstream.prepare(case, repo, private)
    elif case == "ledger-recovery":
        source = HERE / "long_cases" / case
        shutil.copytree(source / "repo", repo, dirs_exist_ok=True)
        shutil.copytree(source / "grader", private, dirs_exist_ok=True)
        prompt = (source / "prompt.txt").read_text()
        command = ["/venv/bin/python", "-m", "pytest", "-q", "/grader/test_outputs.py"]
        reference = {"kind": "shell"}
        info = {"family": "proteus-authored", "immutable_files": [
            k for k in corpus.manifest(repo) if k.startswith(("tests/", "fixtures/", "docs/")) or k == "README.md"]}
    elif case == "go-review":
        import long_go
        prompt, followups, command, reference, info = long_go.prepare(repo, private)
    else:
        import long_repo
        if case not in long_repo.CASES:
            raise ValueError("Unknown case: " + case)
        prompt, command, reference, info = long_repo.prepare(case, repo, private)
    rules = "\nWork locally with the supplied materials. Do not fetch solutions or other resources over the network.\n"
    (path / "prompt.txt").write_text(prompt.strip() + "\n" + rules)
    if followups:
        corpus.write_json(path / "followups.json", followups)
    info.update({"id": case, "repo_hashes": corpus.manifest(repo), "grader_hashes": corpus.manifest(private),
                 "command": command, "reference": reference, "prompt_sha256": corpus.digest(path/"prompt.txt")})
    corpus.write_json(path / "case.json", info)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("cases", nargs="+")
    p.add_argument("--output", type=Path, default=DATA/"long-cases")
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    for name in args.cases:
        if Path(name).name != name or (args.output/name).exists():
            p.error(f"expected new case directory: {name}")
        with tempfile.TemporaryDirectory(dir=args.output) as stage:
            path = Path(stage)/name
            prepare(name, path)
            controls = {}
            for label, ref in [("starter", False), ("gold", True)]:
                result = corpus.grade(path.resolve(), (path/"repo").resolve(), path/(label+".json"), ref)
                controls[label] = result["passed"]
                if result["passed"] != ref or result["timed_out"]:
                    raise ValueError(f"{name} {label} control failed: " + result["output"][-2200:])
            path.rename(args.output/name)
            print(name, json.dumps(controls), flush=True)


if __name__ == "__main__":
    main()
