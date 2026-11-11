"""Three successive requests on the same Go record review library."""
import shutil
import subprocess
from pathlib import Path

import corpus

HERE = Path(__file__).resolve().parent


def prepare(repo, private):
    checkout = corpus.DATA / "polyglot"
    if subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=checkout, text=True).strip() != corpus.PINS["polyglot"]:
        raise ValueError("Unexpected polyglot revision")
    if subprocess.check_output(["git", "status", "--porcelain"], cwd=checkout, text=True):
        raise ValueError("Dirty polyglot checkout")
    for name in ("sgf-parsing", "go-counting"):
        source = checkout / "python/exercises/practice" / name
        for path in source.glob("*.py"):
            shutil.copy2(path, repo / path.name)
            if path.name.endswith("_test.py"):
                shutil.copy2(path, private / path.name)
        shutil.copy2(source / ".docs/instructions.md", repo / (name + ".md"))
        (private / "reference").mkdir(exist_ok=True)
        shutil.copy2(source / ".meta/example.py", private / "reference" / (name.replace("-", "_") + ".py"))
    files = HERE / "long_cases/go-review/grader"
    for path in files.iterdir():
        shutil.copy2(path, private / ("reference" if path.name == "go_review.py" else "") / path.name)
    (private / "conftest.py").write_text("import sys\nsys.path.insert(0, '/app')\n")
    (private / "reference.sh").write_text("set -eu\ncp /grader/reference/*.py /app/\n")
    first = ("Implement the SGF parser described in sgf-parsing.md. Preserve its public API and run the public parser tests. "
             "We'll add board analysis later. For the eventual JSON output remember our agreed wire convention: "
             "the territory counts object is named regions and uses the keys black, white and neutral. "
             "Keep supplied tests and instructions unchanged.")
    turns = [
        "Now implement the territory API in go_counting.py according to go-counting.md and its public tests. "
        "Use the same checkout, keep the parser working and run both public suites.",
        "Integrate both APIs in a new module go_review.py with review(sgf, path) returning a Python dict. "
        "SGF root contains one SZ[n], 1 <= n <= 19. path is a list of zero-based child indices starting at root. "
        "Apply setup properties at the root and along exactly that path in AE, AB, AW order; all are lists "
        "of two lowercase letters (x then y, a is zero); clear/set intersections. Ignore comments. This fixture "
        "uses setup positions only. Report board as an array of n strings using B, W, and . for empty cells; "
        "also include the territory COUNTS in our wire convention from my first message. Return exactly those "
        "two fields. The path [] stops at root. Raise ValueError for an invalid board size, point, or child index "
        "(including negative, non-integer and boolean indices). Neither repeated reviews nor sibling variations "
        "may leak mutations. Demonstrate the integrated API and keep both existing public test suites passing.",
    ]
    return first, turns, ["/venv/bin/python", "-m", "pytest", "-q", "-o", "python_files=*_test.py test_*.py", "/grader"], {"kind": "shell"}, {
        "family":"aider-polyglot-composed", "source_rev":corpus.PINS["polyglot"],
        "source_url":"https://github.com/Aider-AI/polyglot-benchmark", "adaptations":[
            "Two original Exercism exercises plus a new integrated SGF position review, three separately admitted user turns."],
        "immutable_files":[n for n in corpus.manifest(repo) if n.endswith(("_test.py", ".md"))]}
