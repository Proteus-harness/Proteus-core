"""Three longer tasks from the pinned Terminal-Bench 2 source checkout."""
from pathlib import Path
import hashlib
import shutil
import subprocess

HERE = Path(__file__).resolve().parent
CORE = HERE.parents[2]
SOURCE = CORE / "target/performance/coding-harness/tb2"
PIN = "2fd12b88aafdd04a52c298e3940bcb189f9766d6"
CASES = ("write-compressor", "schemelike-metacircular-eval", "db-wal-recovery")


def _manifest(path):
    return {str(p.relative_to(path)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(path.rglob("*")) if p.is_file()}


def prepare(case_id, repo: Path, private: Path):
    """Copy public inputs and private verifier assets into the supplied dirs."""
    if case_id not in CASES:
        raise ValueError(f"Unknown upstream long task {case_id}")
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=SOURCE, text=True).strip()
    if revision != PIN:
        raise ValueError(f"Terminal-Bench source drift: {revision}")
    if subprocess.check_output(["git", "status", "--porcelain"], cwd=SOURCE, text=True):
        raise ValueError("Terminal-Bench source checkout is dirty")
    source = SOURCE / case_id
    repo.mkdir(parents=True, exist_ok=True)
    private.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source / "tests/test_outputs.py", private / "test_outputs.py")
    shutil.copy2(source / "solution/solve.sh", private / "reference.sh")
    changes = ["/app paths in the task prompt become workspace-relative",
               "grader runs in bubblewrap with Python 3.13.9 instead of the upstream Docker image"]
    if case_id == "write-compressor":
        for name in ("decomp.c", "data.txt"):
            shutil.copy2(source / "environment" / name, repo / name)
        subprocess.run(["gcc", "-O3", str(repo / "decomp.c"), "-o", str(repo / "decomp")], check=True)
        for name in ("original-decomp.c", "original-data.txt"):
            shutil.copy2(source / "tests" / name, private / name)
        reference_script = private / "reference.sh"
        reference_script.write_text(reference_script.read_text().replace(
            "rustc main.rs -o main", "rustc -C opt-level=2 -C strip=symbols main.rs -o main"))
        changes.extend(["decomp executable is built from the unchanged upstream C source with the host gcc",
                        "reference Rust compressor is optimized and stripped to stay within the verifier file-size limit"])

    elif case_id == "schemelike-metacircular-eval":
        shutil.copy2(source / "environment/tests/interp.py", repo / "interp.py")
        shutil.copytree(source / "environment/tests/test", repo / "test")
        for name in ("test", "shadow_test"):
            shutil.copytree(source / "tests" / name, private / name)
        test = private / "test_outputs.py"
        text = test.read_text().replace('Path("/tests/test")', 'Path("/grader/test")')
        text = text.replace('Path("/tests/shadow_test")', 'Path("/grader/shadow_test")')
        text = text.replace('''        if not test_files:
            print("No test files found!")
            return 1''', '''        assert test_files, "No authoritative Scheme programs found"''')
        test.write_text(text)
        changes.extend(["verifier resource root /tests becomes /grader",
                        "an empty verifier program list raises AssertionError instead of returning an ignored value"])
    else:
        shutil.copy2(source / "environment/main.db", repo / "main.db")
        shutil.copy2(source / "environment/main.db-wal.encrypted", repo / "main.db-wal")
        verifier = private / "test_outputs.py"
        verifier.write_text(verifier.read_text() + "\n\ndef test_database_is_repaired_and_matches_the_report():\n"
            "    import sqlite3\n"
            "    with sqlite3.connect('file:/app/main.db?mode=ro', uri=True) as connection:\n"
            "        rows = connection.execute('SELECT id,name,value FROM items ORDER BY id').fetchall()\n"
            "    assert len(rows) == 11, 'SQLite must read all WAL changes from the repaired database'\n"
            "    with open('/app/recovered.json') as stream:\n"
            "        recovered = json.load(stream)\n"
            "    assert recovered == [dict(zip(('id','name','value'), row)) for row in rows]\n")
        changes.append("additional read-only SQLite assertion requires the repaired database to expose all rows and match recovered.json")
    prompt = (source / "instruction.md").read_text().replace("/app/", "./").replace("/app", ".")
    prompt += "\nWork in the current directory. Installed system compilers and Python standard library are available.\n"
    provenance = {
        "id": case_id,
        "family": "terminal-bench-2-long",
        "source_rev": PIN,
        "source_url": f"https://github.com/harbor-framework/terminal-bench-2/tree/{PIN}/{case_id}",
        "source_hashes": _manifest(source),
        "grader_files": [str(p.relative_to(private)) for p in sorted(private.rglob("*"))
                         if p.is_file() and p.name != "reference.sh"],
        "immutable_files": ({"db-wal-recovery": [], "write-compressor": ["data.txt", "decomp.c"]}.get(case_id, list(_manifest(repo)))),
        "adaptations": changes,
        "grader_timeout_seconds": 2400 if case_id == "schemelike-metacircular-eval" else 120,
        "environment": {"grader_python": "3.13.9", "pytest": "8.4.1", "network": False},
    }
    if case_id == "write-compressor":
        provenance["environment"]["gcc"] = subprocess.check_output(
            ["/usr/bin/gcc", "--version"], text=True).splitlines()[0]
        provenance["environment"]["reference_rustc"] = subprocess.check_output(
            ["/usr/bin/rustc", "--version"], text=True).strip()
    return prompt, ["/venv/bin/python", "-m", "pytest", "-q", "/grader/test_outputs.py"], {"kind": "shell"}, provenance
