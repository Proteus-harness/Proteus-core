#!/usr/bin/env python3
"""Collect standalone extension packages (ZIPs or a bundled directory)."""
import argparse
import shutil
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent
PACKAGES = ROOT / "packages"


def assemble(destination):
    destination.mkdir(parents=True, exist_ok=True)
    for manifest in sorted(PACKAGES.glob("*/extension.json")):
        package = destination / manifest.parent.name
        shutil.copytree(manifest.parent, package, ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
        shutil.copy2(ROOT / "sdk/proteus_extension.py", package)
        shutil.copy2(ROOT / "LICENSE", package)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "dist")
    parser.add_argument("--bundle", type=Path, help="assemble packages into a new directory instead of ZIPs")
    args = parser.parse_args()
    if args.bundle:
        assemble(args.bundle)
        return
    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as directory:
        assemble(Path(directory))
        for package in sorted(Path(directory).iterdir()):
            archive = args.output / (package.name + ".zip")
            with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as output:
                for path in sorted(package.rglob("*")):
                    if path.is_file(): output.write(path, path.relative_to(package))
            print(archive)


if __name__ == "__main__":
    main()
