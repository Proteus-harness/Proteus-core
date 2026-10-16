"""Workspace search tree and syntax names; independent of the client."""
from pathlib import PurePosixPath


def search_rows(result, row):
    kinds, children = {}, {}
    for entry in result["entries"]:
        path = entry["path"]
        kinds[path] = entry["kind"] == "directory"
        child = path
        for parent in PurePosixPath(path).parents:
            parent = "" if str(parent) == "." else str(parent)
            children.setdefault(parent, set()).add(child)
            if parent: kinds.setdefault(parent, True)
            child = parent
    rows = []
    def visit(parent, depth):
        for path in sorted(children.get(parent, ()), key=lambda path: (not kinds[path], PurePosixPath(path).name.lower())):
            item = row(path, PurePosixPath(path).name, depth, kinds[path])
            if kinds[path]: item["expanded"] = path in children
            rows.append(item)
            visit(path, depth + 1)
    visit("", 0)
    return rows


def language(path):
    name = PurePosixPath(path).name
    if name in ("Cargo.lock", "Pipfile"): return "toml"
    if name in ("Makefile", "Dockerfile", ".bashrc", ".zshrc", ".profile"): return "bash"
    groups = {
        "rust":"rs", "python":"py pyi", "javascript":"js mjs cjs jsx", "typescript":"ts mts cts", "tsx":"tsx",
        "json":"json jsonc jsonl", "toml":"toml", "yaml":"yaml yml", "bash":"sh bash zsh", "c":"c h",
        "cpp":"cc cpp cxx hpp hh hxx", "go":"go", "css":"css scss", "html":"html htm", "sql":"sql", "diff":"diff patch",
    }
    suffix = PurePosixPath(path).suffix.lstrip(".").lower()
    return next((name for name, extensions in groups.items() if suffix in extensions.split()), "text")
