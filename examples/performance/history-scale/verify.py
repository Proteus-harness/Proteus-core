#!/usr/bin/env python3
"""Separate post-measurement process: canonical preservation and workflow replay."""
import argparse
import json
from pathlib import Path
import subprocess

from journal import digest, seed_summary, verify_active


def verify_case(path, mode, preflight=False):
    case = path.parent
    manifest = json.loads(path.read_text())
    results = []
    for config, entry in manifest["entries"].items():
        session = Path(entry["session_dir"])
        for relative, original in entry["canonical_files"].items():
            file = session / relative
            if relative == "journal.jsonl" and mode == "agent-control" and not preflight:
                # Execution appends facts. Its exact original prefix must survive.
                source = Path(entry["source_seed"]).parent / "config"
                # The original bytes are located via the canonical fixture path.
                seed_journals = list(source.rglob("journal.jsonl"))
                if len(seed_journals) != 1:
                    raise ValueError("expected exactly one original seed journal")
                expected = seed_journals[0].read_bytes()
                with file.open("rb") as stream:
                    if stream.read(len(expected)) != expected:
                        raise ValueError("original canonical journal prefix changed")
            elif digest(file) != original:
                raise ValueError(f"canonical seed file changed: {file}")
        if preflight:
            from ui_probe import probe
            seed = json.loads(Path(entry["source_seed"]).read_text())
            cold_ui = probe(manifest["binary"], config, manifest["workspace"], session, seed,
                            case / f"preflight-ui-peer-{entry['peer']}.json", exact=True)
            results.append(dict(peer=entry["peer"], preflight=True, canonical_unchanged=True, cold_ui=cold_ui))
            continue
        if mode == "resume-idle":
            results.append(dict(peer=entry["peer"], rows=entry["rows"], canonical_unchanged=True))
            continue
        seed = json.loads(Path(entry["source_seed"]).read_text())
        verified = verify_active(session, seed, manifest["rounds"])
        journal = session / "journal.jsonl"
        before = digest(journal)
        for turn_index, turn in enumerate(verified["settled_turn_ids"]):
            command = [manifest["binary"], "--config", config, "--cwd", manifest["workspace"],
                       "replay", "workflow", str(journal), "--turn-id", turn, "--json"]
            replay = subprocess.run(command, cwd=manifest["workspace"], capture_output=True, text=True, timeout=120)
            (case / f"replay-{entry['peer']}-{turn_index}.log").write_text(replay.stdout + replay.stderr)
            if replay.returncode:
                raise ValueError(f"workflow replay failed: {replay.stderr[-2000:]}")
            payload = json.loads(replay.stdout)
            if not payload["comparison"]["matched"] or not payload["source_journal_unchanged"] or digest(journal) != before:
                raise ValueError("workflow replay mismatch or source mutation")
        if seed.get("seed_compaction"):
            from ui_probe import probe
            verified["cold_ui"] = probe(manifest["binary"], config, manifest["workspace"], session, seed,
                                        case / f"post-active-ui-peer-{entry['peer']}.json")
        results.append(dict(peer=entry["peer"], **verified, replay_matched=True, journal_sha256=before))
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    seed = commands.add_parser("seed")
    seed.add_argument("session", type=Path)
    seed.add_argument("rows", type=int)
    seed.add_argument("text_repeats", type=int)
    seed.add_argument("output", type=Path)
    seed.add_argument("--model-history-rows", type=int)
    case = commands.add_parser("case")
    case.add_argument("manifest", type=Path)
    case.add_argument("mode", choices=("resume-idle", "agent-control"))
    case.add_argument("output", type=Path)
    preflight = commands.add_parser("preflight")
    preflight.add_argument("manifest", type=Path)
    preflight.add_argument("output", type=Path)
    args = parser.parse_args()
    if args.command == "seed":
        result = seed_summary(args.session, args.rows, args.text_repeats, args.model_history_rows)
    else:
        result = verify_case(args.manifest, args.mode if args.command == "case" else "agent-control",
                             preflight=args.command == "preflight")
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")


if __name__ == "__main__":
    main()
