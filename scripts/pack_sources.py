#!/usr/bin/env python3
"""Pack all project source code into a single file.

The output file contains:
  1. A header with metadata (root, timestamp, file count).
  2. A rendered file tree of everything that gets packed.
  3. The full text of every file, each wrapped in an unambiguous splitter.

Files ignored by .gitignore (and .git/info/exclude, core.excludesFile) are
never included: the file list comes from git itself. If the directory is not a
git repository, a conservative fallback walk is used that skips common build /
dependency directories.

On top of that, an explicit exclude list is applied (tracked files gitignore
cannot touch). By default lockfiles are dropped — they are bulky and rarely
needed in a source pack. Patterns are fnmatch globs matched against both the
relative path and the bare filename, so `Cargo.lock` matches at any depth.

Self-awareness: the output file is never packed into itself, and the packer
script itself is skipped by default (it is tooling, not project source) — pass
--include-self to bundle it anyway.

A leading UTF-8 BOM (U+FEFF, often added by Windows editors) is stripped from
the packed text — it is invisible in editors but renders as a stray glyph in
the bundle. Files that carry one are tagged "utf-8 bom" in their META line; the
originals on disk are left untouched.

Usage:
  python3 scripts/pack_sources.py                          # -> ./sources-pack.txt
  python3 scripts/pack_sources.py -o bundle.txt
  python3 scripts/pack_sources.py --exclude '*.pb' --exclude generated/
  python3 scripts/pack_sources.py --no-default-excludes     # keep Cargo.lock
  python3 scripts/pack_sources.py --include-self            # bundle the packer too
"""

from __future__ import annotations

import argparse
import datetime as _dt
import fnmatch
import mimetypes
import os
import subprocess
import sys
from pathlib import Path, PurePosixPath

SPLITTER = "#" * 80
DEFAULT_OUTPUT_NAME = "sources-pack.txt"

# Applied on top of .gitignore; fnmatch globs vs. relative path or basename.
DEFAULT_EXCLUDE_PATTERNS = ("Cargo.lock",)

# Used only when git is unavailable (fallback walk).
FALLBACK_SKIP_DIRS = {
    ".git",
    ".venv",
    "venv",
    "__pycache__",
    "node_modules",
    "target",
    "dist",
    "build",
    ".idea",
    ".vscode",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Pack project source code into one file with a file tree "
        "and splitters between files. Respects .gitignore."
    )
    parser.add_argument(
        "--root",
        type=Path,
        default=None,
        help="Project root (default: nearest git root, else current directory).",
    )
    parser.add_argument(
        "-o",
        "--output",
        type=Path,
        default=None,
        help=f"Output file (default: <root>/{DEFAULT_OUTPUT_NAME}).",
    )
    parser.add_argument(
        "--max-file-bytes",
        type=int,
        default=1_000_000,
        help="Files larger than this are listed but skipped (default: 1000000). "
        "Use 0 for no limit.",
    )
    parser.add_argument(
        "--exclude",
        action="append",
        default=[],
        metavar="PATTERN",
        help="Additional fnmatch glob to exclude, e.g. --exclude '*.lock'. "
        "Matched against the relative path and the filename. Repeatable.",
    )
    parser.add_argument(
        "--no-default-excludes",
        action="store_true",
        help="Do not apply the built-in excludes (currently: "
        + ", ".join(DEFAULT_EXCLUDE_PATTERNS)
        + ").",
    )
    parser.add_argument(
        "--include-self",
        action="store_true",
        help="Also pack this script (excluded by default).",
    )
    parser.add_argument(
        "--include-binary",
        action="store_true",
        help="Also embed binary files, base64-encoded (default: skip, note only).",
    )
    parser.add_argument(
        "--tree-only",
        action="store_true",
        help="Only print the file tree, do not write the output file.",
    )
    return parser.parse_args()


def find_git_root(start: Path) -> Path | None:
    try:
        result = subprocess.run(
            ["git", "-C", str(start), "rev-parse", "--show-toplevel"],
            capture_output=True,
            text=True,
            check=False,
        )
    except FileNotFoundError:
        return None
    if result.returncode != 0:
        return None
    return Path(result.stdout.strip())


def list_files_git(root: Path) -> list[str]:
    """All files git knows about: tracked + untracked but NOT ignored."""
    result = subprocess.run(
        ["git", "-C", str(root), "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        capture_output=True,
        check=True,
    )
    paths = [p for p in result.stdout.decode("utf-8", "surrogateescape").split("\0") if p]
    return sorted(paths)


def list_files_fallback(root: Path) -> list[str]:
    paths: list[str] = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if d not in FALLBACK_SKIP_DIRS)
        for name in sorted(filenames):
            full = Path(dirpath) / name
            rel = full.relative_to(root).as_posix()
            paths.append(rel)
    return sorted(paths)


def is_excluded(rel_path: str, patterns: list[str]) -> bool:
    """True if rel_path matches any exclude glob (path or bare filename)."""
    name = PurePosixPath(rel_path).name
    return any(
        fnmatch.fnmatch(rel_path, pattern) or fnmatch.fnmatch(name, pattern)
        for pattern in patterns
    )


def is_binary(sample: bytes) -> bool:
    if b"\0" in sample:
        return True
    # Heuristic for invalid UTF-8 text (e.g. compressed data without NUL early).
    try:
        sample.decode("utf-8")
    except UnicodeDecodeError:
        return True
    return False


def guess_kind(path: str) -> str:
    mime, _ = mimetypes.guess_type(path)
    return mime or "text/plain"


def build_tree(paths: list[str], root_name: str) -> str:
    """Render an ASCII tree for the given relative posix paths."""
    # node: dict[str, node]; files are leaf dicts; track which keys are files.
    tree: dict = {}
    files: set[tuple[str, ...]] = set()
    for p in paths:
        parts = PurePosixPath(p).parts
        node = tree
        for i, part in enumerate(parts):
            node = node.setdefault(part, {})
            if i == len(parts) - 1:
                files.add(parts)

    lines: list[str] = [root_name + "/"]

    def walk(node: dict, prefix: str, trail: tuple[str, ...]) -> None:
        def sort_key(kv: tuple[str, dict]) -> tuple[int, str]:
            name, child = kv
            is_dir = (trail + (name,)) not in files and bool(child)
            return (0 if is_dir else 1, name.lower())  # directories first

        entries = sorted(node.items(), key=sort_key)
        for index, (name, child) in enumerate(entries):
            last = index == len(entries) - 1
            connector = "└── " if last else "├── "
            is_dir = (trail + (name,)) not in files and bool(child)
            lines.append(f"{prefix}{connector}{name}{'/' if is_dir else ''}")
            if is_dir:
                extension = "    " if last else "│   "
                walk(child, prefix + extension, trail + (name,))

    walk(tree, "", ())
    return "\n".join(lines)


def render_file_section(rel_path: str, abs_path: Path, include_binary: bool, max_bytes: int) -> str:
    size = abs_path.stat().st_size
    with abs_path.open("rb") as fh:
        sample = fh.read(8192)
    binary = is_binary(sample)
    has_bom = sample.startswith(b"\xef\xbb\xbf")

    meta_bits = [f"{size} bytes", "binary" if binary else guess_kind(rel_path)]
    if has_bom:
        meta_bits.append("utf-8 bom")  # U+FEFF present in the source file
    header = [
        SPLITTER,
        f"# FILE: {rel_path}",
        f"# META: {' | '.join(meta_bits)}",
        SPLITTER,
    ]

    if max_bytes and size > max_bytes:
        header.append(f"[SKIPPED: file exceeds --max-file-bytes={max_bytes}]")
    elif binary and not include_binary:
        header.append("[SKIPPED: binary file — rerun with --include-binary to embed base64]")
    else:
        data = abs_path.read_bytes()
        if binary:
            import base64

            encoded = base64.b64encode(data).decode("ascii")
            body = "\n".join(encoded[i : i + 76] for i in range(0, len(encoded), 76))
            header.append("[BASE64-ENCODED BINARY]")
            header.append(body)
        else:
            text = data.decode("utf-8", "surrogateescape")
            if text.startswith("\ufeff"):
                text = text[1:]  # drop the invisible BOM marker from the pack
            header.append(text.rstrip("\n"))

    header.append("")
    header.append(f"# END FILE: {rel_path}")
    header.append(SPLITTER)
    return "\n".join(header)


def main() -> int:
    args = parse_args()

    root = args.root.resolve() if args.root else None
    if root is None:
        root = find_git_root(Path.cwd()) or Path.cwd()
    if not root.is_dir():
        print(f"error: root is not a directory: {root}", file=sys.stderr)
        return 2

    git_root = find_git_root(root)
    if git_root is not None:
        rel_paths = list_files_git(root if root == git_root else git_root)
        if root != git_root:
            prefix = root.relative_to(git_root).as_posix().rstrip("/") + "/"
            rel_paths = [p for p in rel_paths if p.startswith(prefix)]
        base = git_root
    else:
        rel_paths = list_files_fallback(root)
        base = root

    root_name = base.name

    # Explicit excludes (tracked files that .gitignore cannot touch).
    exclude_patterns = [] if args.no_default_excludes else list(DEFAULT_EXCLUDE_PATTERNS)
    exclude_patterns += args.exclude
    if exclude_patterns:
        rel_paths = [p for p in rel_paths if not is_excluded(p, exclude_patterns)]

    # Never pack the output file into itself, and skip the packer script itself
    # (tooling, not project source) unless --include-self is given.
    script_rel: str | None = None
    try:
        script_rel = Path(__file__).resolve().relative_to(base).as_posix()
    except ValueError:
        pass  # script lives outside the project root — nothing to skip
    if script_rel and not args.include_self and script_rel in rel_paths:
        rel_paths.remove(script_rel)

    output: Path | None = None
    if not args.tree_only:
        output = (args.output or (root / DEFAULT_OUTPUT_NAME)).resolve()
        try:
            out_rel = output.relative_to(base).as_posix()
        except ValueError:
            out_rel = None
        if out_rel and out_rel in rel_paths:
            rel_paths.remove(out_rel)

    tree_text = build_tree(rel_paths, root_name)

    if args.tree_only:
        print(tree_text)
        print(f"\n{len(rel_paths)} files would be packed.")
        return 0

    now = _dt.datetime.now().astimezone().isoformat(timespec="seconds")
    parts = [
        SPLITTER,
        "# SOURCE PACK",
        f"# Root:    {base}",
        f"# Created: {now}",
        f"# Files:   {len(rel_paths)}",
        "# Note:    .gitignore'd files and the packer script are excluded",
        SPLITTER,
        "",
        "=" * 32 + " FILE TREE " + "=" * 37,
        tree_text,
        "",
        "=" * 28 + " FILE CONTENTS " + "=" * 37,
        "",
    ]

    for rel in rel_paths:
        parts.append(render_file_section(rel, base / rel, args.include_binary, args.max_file_bytes))
        parts.append("")

    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text("\n".join(parts), encoding="utf-8", errors="surrogateescape")

    total_bytes = output.stat().st_size
    print(f"Packed {len(rel_paths)} files -> {output} ({total_bytes} bytes)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
