"""Resolve separate source repositories for builds and process fixtures."""
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent.parent


def resolve(name):
    spec = json.loads((ROOT / "repositories.json").read_text())[name]
    override = os.environ.get(spec["env"])
    if override:
        path = Path(override).expanduser().resolve()
        if not (path / spec["marker"]).is_file():
            raise SystemExit(f"Invalid {spec['env']}: {path}")
        return path
    local = (ROOT / spec["local"]).resolve()
    if (local / spec["marker"]).is_file():
        return local
    path = ROOT / "target/sources" / name
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.exists():
        subprocess.run(["git", "clone", "--no-checkout", spec["url"], str(path)], check=True)
    present = subprocess.run(["git", "cat-file", "-e", spec["rev"] + "^{commit}"], cwd=path,
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0
    if not present:
        subprocess.run(["git", "fetch", "origin", spec["rev"]], cwd=path, check=True)
    subprocess.run(["git", "checkout", "--detach", spec["rev"]], cwd=path, check=True,
                   stdout=subprocess.DEVNULL)
    return path
