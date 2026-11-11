"""Paired coding tasks, plus a fresh repeat of the same calibration task."""
import argparse
import concurrent.futures
import json
from pathlib import Path
import subprocess
import sys

HERE = Path(__file__).resolve().parent
CORE = HERE.parents[2]
DATA = CORE / "target/performance/coding-harness"
ARMS = ["pi", "proteus-pi", "codex", "proteus-codex", "opencode", "proteus-opencode"]
MODELS = {"flash": ("opencode-go/deepseek-v4.1-flash", "high"), "sol": ("gpt-6.1-sol", "high"), "terra": ("gpt-6.1-terra", "xhigh")}
CASES = ["variable-length-quantity", "cancel-async-tasks", "forth", "poker", "log-summary-date-ranges", "dominoes"]


def run(job):
    root, model, case, attempt, arm = job
    case_dir = DATA / "cases" / case
    output = root / f"{model}_{case}_{attempt}_{arm}"
    if model == "flash" and arm == "codex":
        output.mkdir(mode=0o700)
        (output/"unavailable.json").write_text(json.dumps({
            "available": False, "model": MODELS[model][0], "arm": arm,
            "reason": "Go Responses endpoint probe rejected (HTTP 403); native Codex model transport unverified",
            "evidence": str(DATA/"flash-responses-probe.json"),
            "gpt_fallback": False,
        }, indent=2)+"\n")
        print("UNAVAILABLE native Codex: Flash Go Responses endpoint rejected HTTP 403", flush=True)
        return False
    if output.exists():
        raise FileExistsError(output)
    native = not arm.startswith("proteus-")
    agent = arm if native else "proteus"
    argv = [sys.executable, "-B", str(HERE/"runner.py"), "--agent", agent, "--case-dir", str(case_dir),
            "--output", str(output), "--model", MODELS[model][0], "--reasoning", MODELS[model][1]]
    if not native:
        argv.extend(["--base-config", str(CORE/"configs"/(arm.removeprefix("proteus-")+".config.toml"))])
    elif arm == "pi":
        argv.extend(["--binary", str(Path.home()/".local/bin/pi")])
    elif arm == "opencode":
        argv.extend(["--binary", str(Path.home()/".opencode/bin/opencode"), "--auth-file", str(Path.home()/".pi/agent/auth.json")])
    else:
        argv.extend(["--binary", "/usr/lib/chatgpt/resources/codex"])
    subprocess.run(argv, cwd=CORE)
    if not (output/"result.json").exists():
        raise RuntimeError(f"runner did not record result: {output}")
    result = json.loads((output/"result.json").read_text())
    grade = subprocess.run([sys.executable, "-B", str(HERE/"corpus.py"), "grade", str(case_dir),
                            "--workspace", str(output/"workspace"), "--output", str(output/"grading.json")],
                           stdout=subprocess.DEVNULL)
    good = bool(result.get("pipeline_ok")) and grade.returncode == 0
    print(f"{'PASS' if good else 'FAIL'} {output.name} exit={result['returncode']} grader={grade.returncode}", flush=True)
    return good


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arms", choices=ARMS, nargs="+", required=True)
    parser.add_argument("--models", choices=MODELS, nargs="+", default=["flash"])
    parser.add_argument("--cases", choices=CASES, nargs="+", default=CASES)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--jobs", type=int, choices=range(1,9), default=4)
    parser.add_argument("--repeat", action="store_true", help="repeat first case in a fresh session")
    args = parser.parse_args()
    if not (DATA/"cases").is_dir():
        parser.error("prepare corpus first")
    args.output.mkdir(parents=True, exist_ok=True)
    cases = [(c,1) for c in args.cases] + ([(args.cases[0],2)] if args.repeat else [])
    jobs = []
    for index, (case, attempt) in enumerate(cases):
        for model in args.models:
            shift = index % len(args.arms)
            for arm in args.arms[shift:]+args.arms[:shift]:
                jobs.append((args.output.resolve(),model,case,attempt,arm))
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        results = list(pool.map(run,jobs))
    return 0 if all(results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
