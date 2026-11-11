"""Repository maintenance cases built from a fixed public Proteus-core revision."""
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[3]
FIXTURES = Path(__file__).with_name("long_repo_fixtures")
REVISION = "879183f3853ce469346b19634e9440c6d7d7cd0f"
CASES = ("journal-segments", "sampler-phase-report")


def prepare(case_id, repo, private):
    if case_id not in CASES:
        raise ValueError(case_id)
    paths = ["LICENSE", "examples/performance/README.md", "examples/performance/summarize.py",
             "examples/performance/common/README.md", "examples/performance/common/proc_sampler.py",
             "examples/performance/common/test_proc_sampler.py", "examples/performance/common/__init__.py",
             "examples/performance/history-scale/README.md", "examples/performance/history-scale/journal.py",
             "examples/performance/code-mode/measurements.py", "examples/performance/code-mode/README.md"]
    for name in paths:
        target = repo / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(subprocess.check_output(["git", "show", f"{REVISION}:{name}"], cwd=ROOT))
    source = FIXTURES / case_id
    shutil.copytree(source / "repo", repo, dirs_exist_ok=True)
    shutil.copytree(source / "grader", private, dirs_exist_ok=True)
    prompt = (source / "prompt.txt").read_text()
    return (prompt, ["/venv/bin/python", "-m", "pytest", "-q", "/grader/test_outputs.py"],
            {"kind": "shell"}, {"source_url": "https://github.com/Proteus-harness/Proteus-core",
             "source_rev": REVISION, "kind": "repository feature", "source_paths": paths,
             "immutable_files": ["SOURCE.md", "examples/performance/common/test_proc_sampler.py"],
             "license": "MIT", "task_origin": "New maintenance requirements on pinned public source"})
