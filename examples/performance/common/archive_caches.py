#!/usr/bin/env python3
"""Archive verified derived SQLite caches from a completed benchmark only."""
import argparse
import fnmatch
import hashlib
import io
import json
import os
from pathlib import Path
import stat
import tarfile


def sha(stream):
    return hashlib.file_digest(stream, "sha256").hexdigest()


def digest(path):
    with path.open("rb") as stream:
        return sha(stream)


def completed(root):
    report = json.loads((root / "report.json").read_text())
    if report.get("status") != "passed":
        raise ValueError("benchmark report must be passed; stop all benchmark processes first")
    if "runs" in report:
        parameters = json.loads((root / "invocation.json").read_text())["arguments"]
        rows = report["runs"]
        expected = {(count, repeat) for count in parameters["messages"] for repeat in range(parameters["repeats"])}
        actual = [(row["messages_per_session"], row["repeat"]) for row in rows]
    else:
        parameters = json.loads((root / "manifest.json").read_text())["parameters"]
        rows = report["results"]
        expected = {(count, repeat, cache) for count in parameters["messages"]
                    for repeat in range(parameters["repeats"]) for cache in ("first-app-cache", "restart")}
        actual = [(row["messages"], row["repeat"], row["cache"]) for row in rows]
    if not expected or len(actual) != len(expected) or set(actual) != expected or any(row.get("status") != "passed" for row in rows):
        raise ValueError("completed cases do not match the full requested benchmark matrix")


def fingerprint(path):
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or path.resolve() != path:
        raise ValueError(f"cache is no longer a regular non-symlink file: {path}")
    return info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns


def archive(root, output, remove=False):
    completed(root)
    sidecar = Path(str(output) + ".manifest.json")
    if os.path.lexists(output) or os.path.lexists(sidecar):
        raise FileExistsError("archive and sidecar must both be new paths")
    files, identities = [], {}
    for directory, folders, names in os.walk(root, followlinks=False):
        folders[:] = sorted(name for name in folders if name != "blobs" and not (Path(directory) / name).is_symlink())
        for name in sorted(names):
            path = Path(directory) / name
            if not fnmatch.fnmatchcase(name, ".read-*.sqlite*") or not stat.S_ISREG(path.lstat().st_mode):
                continue
            relative = path.relative_to(root).as_posix()
            identity = fingerprint(path)
            identities[relative] = identity
            files.append(dict(path=relative, bytes=identity[2], sha256=digest(path)))
    if not files:
        raise ValueError("no regular derived .read-*.sqlite* caches found")
    manifest = dict(version=1, root=str(root), report_sha256=digest(root / "report.json"), files=files)
    payload = (json.dumps(manifest, ensure_ascii=False, indent=2) + "\n").encode()
    internal = "CACHE-MANIFEST.json"
    with output.open("xb") as raw:
        with tarfile.open(fileobj=raw, mode="w:gz", compresslevel=1, dereference=True) as target:
            info = tarfile.TarInfo(internal)
            info.size = len(payload)
            target.addfile(info, io.BytesIO(payload))
            for entry in files:
                path = root / entry["path"]
                if fingerprint(path) != identities[entry["path"]]:
                    raise ValueError(f"cache changed while archiving: {path}")
                target.add(path, arcname=entry["path"], recursive=False)
        raw.flush()
        os.fsync(raw.fileno())
    expected = {entry["path"]: (entry["bytes"], entry["sha256"]) for entry in files}
    expected[internal] = (len(payload), hashlib.sha256(payload).hexdigest())
    seen = set()
    with tarfile.open(output, "r:gz") as source:
        for member in source:
            if not member.isfile() or member.name not in expected or member.name in seen:
                raise ValueError("unexpected, duplicate or nonregular archive entry")
            with source.extractfile(member) as stream:
                if (member.size, sha(stream)) != expected[member.name]:
                    raise ValueError(f"archive content mismatch: {member.name}")
            seen.add(member.name)
    if seen != set(expected):
        raise ValueError("archive is missing expected entries")
    for entry in files:
        path = root / entry["path"]
        if fingerprint(path) != identities[entry["path"]] or digest(path) != entry["sha256"]:
            raise ValueError(f"live cache changed before removal: {path}")
    manifest.update(archive=str(output), archive_sha256=digest(output), archive_bytes=output.stat().st_size,
                    verified=True, remove_requested=remove)
    with sidecar.open("x") as stream:
        json.dump(manifest, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())
    if remove:
        for entry in files:
            path = root / entry["path"]
            if fingerprint(path) != identities[entry["path"]]:
                raise ValueError(f"cache changed immediately before unlink: {path}")
            path.unlink()
    return dict(file_count=len(files), original_bytes=sum(entry["bytes"] for entry in files), **manifest)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--remove", action="store_true", help="unlink verified derived caches after durable archive creation")
    args = parser.parse_args()
    result = archive(args.root.resolve(strict=True), args.output.absolute(), args.remove)
    print(json.dumps({key: result[key] for key in ("archive", "archive_sha256", "archive_bytes", "original_bytes", "remove_requested")}))


if __name__ == "__main__":
    main()
