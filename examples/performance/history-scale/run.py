#!/usr/bin/env python3
"""Synthetic per-session history scale: ten resumed HTTP backends or AgentControl peers."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import traceback

from fixtures import HERE, prepare_case, prepare_fixtures, write_json
from journal import digest

ROOT = HERE.parents[2]


def run_verifier(args, case, command, prefix):
    from active import abort_unobserved_group
    with (case / f"{prefix}.stdout.log").open("w") as output, (case / f"{prefix}.stderr.log").open("w") as error:
        process = subprocess.Popen(command, cwd=args.fixtures / "workspace", env=args.env,
                                   stdout=output, stderr=error, start_new_session=True)
        try:
            process.wait(timeout=args.timeout)
        except BaseException:
            if process.returncode is None:
                abort_unobserved_group(process)
            raise
    if process.returncode:
        raise RuntimeError(f"{prefix} failed: {(case / f'{prefix}.stderr.log').read_text()[-3000:]}")


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("resume-idle", "agent-control"), default="resume-idle")
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--module", type=Path, required=True)
    parser.add_argument("--seeder", type=Path, required=True)
    parser.add_argument("--driver", type=Path, default=ROOT / "target/release/examples/multiagent_perf")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--fixtures", "--seed-base", dest="fixtures", type=Path,
                        help="immutable shared fixtures/workspace for identical before/after seed bytes (default OUTPUT/fixtures)")
    parser.add_argument("--link-fixtures", action="store_true",
                        help="resume-idle only: hardlink immutable canonical inputs; still verify all bytes after the run")
    parser.add_argument("--messages", type=int, nargs="+", default=[5000, 50000], help="messages in EACH peer session")
    parser.add_argument("--model-history-rows", type=int,
                        help="opt-in synthetic tail compaction: preserve all display messages, retain this many model-history messages")
    parser.add_argument("--peers", type=int, default=10)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--rounds", type=int, default=3, help="active AgentControl turns per peer")
    parser.add_argument("--idle-seconds", type=float, default=10)
    parser.add_argument("--text-repeats", type=int, default=4)
    parser.add_argument("--payload-kib", type=int, default=32)
    parser.add_argument("--delay-ms", type=int, default=500)
    parser.add_argument("--interval", type=float, default=0.2)
    parser.add_argument("--timeout", type=float, default=600)
    parser.add_argument("--max-pss-mib", type=int, default=6144)
    args = parser.parse_args()
    if args.link_fixtures and args.mode != "resume-idle":
        parser.error("--link-fixtures is allowed only for read-only resume-idle")
    if not 1 <= args.peers <= 10 or not 1 <= args.repeats <= 20 or not 1 <= args.rounds <= 20:
        parser.error("peers must be 1..10, repeats/rounds 1..20")
    if not args.messages or len(set(args.messages)) != len(args.messages) or not all(1 <= rows <= 50000 for rows in args.messages):
        parser.error("messages must be distinct counts in 1..50000")
    if args.model_history_rows is not None and not 1 <= args.model_history_rows <= min(args.messages):
        parser.error("model-history-rows must be 1..the smallest requested display message count")
    if not (1 <= args.text_repeats <= 8 and 1 <= args.payload_kib <= 128 and 1 <= args.delay_ms <= 5000):
        parser.error("fixture text/payload/delay outside bounded range")
    if not (0.05 <= args.interval <= 5 and 1 <= args.timeout <= 600 and 0 < args.idle_seconds <= 60 and 1 <= args.max_pss_mib <= 8192):
        parser.error("sampling interval, timeout, idle time or memory bound outside supported range")
    for name in ("binary", "module", "seeder") + (("driver",) if args.mode == "agent-control" else ()):
        path = getattr(args, name).resolve(strict=True)
        if not path.is_file() or not os.access(path, os.X_OK):
            parser.error(f"{name} must be an executable file: {path}")
        setattr(args, name, path)
    args.output = args.output.resolve()
    args.fixtures = (args.fixtures or args.output / "fixtures").resolve()
    return args


def main():
    args = arguments()
    args.output.mkdir(parents=True, exist_ok=False)  # Never overwrite evidence.
    args.env = os.environ.copy()
    for key, directory in (("PROTEUS_CONFIG_HOME", "control-config"), ("PROTEUS_HOME", "control-home"),
                           ("XDG_CONFIG_HOME", "xdg/config"), ("XDG_DATA_HOME", "xdg/data"),
                           ("XDG_CACHE_HOME", "xdg/cache")):
        path = args.output / directory
        path.mkdir(parents=True)
        args.env[key] = str(path)
    args.env.update(NO_PROXY="localhost,127.0.0.1,::1", no_proxy="localhost,127.0.0.1,::1")
    names = ["binary", "module", "seeder"] + (["driver"] if args.mode == "agent-control" else [])
    binaries = {name: dict(path=str(getattr(args, name)), sha256=digest(getattr(args, name))) for name in names}
    sources = {str(path.relative_to(ROOT)): digest(path) for path in sorted(HERE.glob("*.py"))}
    driver_source = HERE.parent / "multi-agent/driver.rs"
    sources[str(driver_source.relative_to(ROOT))] = digest(driver_source)
    configuration = {key: str(value) if isinstance(value, Path) else value
                     for key, value in vars(args).items() if key != "env"}
    write_json(args.output / "invocation.json", dict(arguments=configuration, binaries=binaries, sources=sources,
               allocator_environment={key: args.env.get(key) for key in ("MALLOC_ARENA_MAX", "MALLOC_TRIM_THRESHOLD_", "GLIBC_TUNABLES")}))
    fixtures = prepare_fixtures(args)
    results = []
    if args.mode == "resume-idle":
        from resume_idle import run
    else:
        from active import run
    for repeat in range(args.repeats):
        for rows in args.messages:
            case = args.output / f"messages-{rows}-peers-{args.peers}-repeat-{repeat}"
            result = dict(messages_per_session=rows, model_history_rows=args.model_history_rows or rows,
                          history_mode="compacted_tail" if args.model_history_rows is not None else "full",
                          peers=args.peers, repeat=repeat, mode=args.mode,
                          path=str(case), status="failed")
            try:
                manifest = prepare_case(args, fixtures, case, rows)
                if args.mode == "agent-control" and args.model_history_rows is not None:
                    run_verifier(args, case, [sys.executable, str(HERE / "verify.py"), "preflight", str(case / "manifest.json"),
                                             str(case / "preflight.json")], "preflight")
                measurement = run(args, case, manifest)
                run_verifier(args, case, [sys.executable, str(HERE / "verify.py"), "case", str(case / "manifest.json"),
                                         args.mode, str(case / "verification.json")], "verification")
                result.update(status="passed", measurement=measurement,
                              verification=json.loads((case / "verification.json").read_text()))
            except Exception as error:
                case.mkdir(parents=True, exist_ok=True)
                (case / "failure.log").write_text(traceback.format_exc())
                result["error"] = repr(error)
                if (case / "measurement.json").exists():
                    result["measurement"] = json.loads((case / "measurement.json").read_text())
            write_json(case / "report.json", result)
            results.append(result)
            report = dict(status=("running" if len(results) < args.repeats * len(args.messages) else
                                  "passed" if all(item["status"] == "passed" for item in results) else "failed"),
                          mode=args.mode, model_history_rows=args.model_history_rows,
                          history_mode="compacted_tail" if args.model_history_rows is not None else "full",
                          binaries=binaries, sources=sources, fixtures=str(args.fixtures / "fixtures.json"),
                          scope=("Each session preserves the full requested durable display history and an explicitly compacted model-history tail; "
                                 if args.model_history_rows is not None else "Each session contains the full requested uncompacted model/display history; ") +
                                "OS caches are uncontrolled/warm. "
                                "resume-idle measures independent read-only HTTP Core backends; agent-control measures real spawn/wait/resume "
                                "with fake model and read_file. Compacted active cases have untimed cold-UI preflight and post-turn probes; "
                                "preflight warms derived read caches before active measurement. Sampled peaks/last-observed counters are lower bounds.",
                          runs=results)
            write_json(args.output / "report.json", report)
            print(json.dumps({key: result[key] for key in ("messages_per_session", "peers", "repeat", "mode", "status")}, ensure_ascii=False), flush=True)
    return 0 if all(result["status"] == "passed" for result in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
