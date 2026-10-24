#!/usr/bin/env python3
"""Fails if the repo (or the staged commit) contains anything from the original game.

A remake repo only ever holds new code. Game data is read from the player's own copy at
runtime, so a game file, a save, or decompiled game code in the repo is a bug.

Adapt the GAME SETTINGS block for each game (container extensions, save magic bytes, the
game's own namespaces), then copy this file to tools/ and the pre-commit hook to .githooks/.

Usage:
  python tools/check_no_game_files.py            # every tracked file
  python tools/check_no_game_files.py --staged   # what is about to be committed
  python tools/check_no_game_files.py --commits SHA...   # files each commit adds or changes (pre-push)

Symlinks (git mode 120000) are rejected in every mode: a link can point at the game install.
"""
from __future__ import annotations

import re
import subprocess
import sys

# ---- GAME SETTINGS (Dead Island 2 / Unreal Engine 5) ---------------------------------------
MAX_BYTES = 5 * 1024 * 1024

# File names that only come from the game or a build (archives, sound banks, saves, binaries).
BLOCKED_NAME = re.compile(
    r"(\.(pak|ucas|utoc|uasset|uexp|umap|ubulk|bnk|wem|glb|sav|dll|exe|pdb|so)$)"
    r"|(^|/)(aes_key\.txt|exe_strings\.txt|utoc_headers\.csv)$",
    re.IGNORECASE,
)

# Magic bytes at the start of files that belong to the game (saves, profiles, custom containers).
# Example for Slime Rancher: rb"\x06SRGAME", rb"\x04SRPF".
GAME_MAGIC: list[bytes] = [b"-==--==--==--==-", b"\xc1\x83\x2a\x9e"]  # .utoc header, .uasset tag

# The game's own code namespaces/identifiers, which only appear in decompiled code.
# Example for Slime Rancher: r"MonomiPark".
GAME_NAMESPACES: list[str] = []
# ----------------------------------------------------------------------------------------------

DECOMPILED_MARKERS = [
    re.compile(rb"ICSharpCode\.Decompiler"),
    re.compile(rb"<Private" rb"ImplementationDetails>"),  # split so this file passes its own check
    re.compile(rb"//\s*Decompiled with", re.IGNORECASE),
    re.compile(rb"/\*\s*WARNING: Decompiled", re.IGNORECASE),  # Ghidra
] + [re.compile(rb"^\s*(namespace|using|package|import)\s+" + re.escape(n.encode()) + rb"\b", re.MULTILINE)
     for n in GAME_NAMESPACES]


def git(*args: str) -> bytes:
    return subprocess.run(["git", *args], check=True, capture_output=True).stdout


def files_to_check(staged: bool, commit: str | None = None) -> list[tuple[str, str]]:
    """(git mode, path) of every file added or changed by the commit / the index, or tracked."""
    if commit or staged:
        if commit:
            out = git("diff-tree", "-r", "--root", "--no-commit-id", "--raw", "--diff-filter=ACMRT", "-z", commit)
        else:
            out = git("diff", "--cached", "--raw", "--diff-filter=ACMRT", "-z")
        parts = out.decode("utf-8").split("\0")
        files = []
        i = 0
        while i < len(parts) - 1:
            meta = parts[i].split()
            if not meta:
                i += 1
                continue
            new_mode, status = meta[1], meta[4]
            # Renames and copies carry two paths; the second is the new one.
            path = parts[i + 2] if status[0] in "RC" else parts[i + 1]
            files.append((new_mode, path))
            i += 3 if status[0] in "RC" else 2
        return files
    out = git("ls-files", "-s", "-z")
    files = []
    for entry in out.decode("utf-8").split("\0"):
        if entry:
            meta, path = entry.split("\t", 1)
            files.append((meta.split()[0], path))
    return files


def read(path: str, staged: bool, commit: str | None = None) -> bytes:
    if commit:
        return git("show", f"{commit}:{path}")
    if staged:
        return git("show", f":{path}")
    with open(path, "rb") as f:
        return f.read()


def is_unity_serialized(data: bytes) -> bool:
    if len(data) < 64:
        return False
    fmt = int.from_bytes(data[8:12], "big")
    return 9 <= fmt <= 30 and re.search(rb"\d{4}\.\d+\.\d+[abfp]\d+\0", data[16:64]) is not None


def problems_in(path: str, data: bytes) -> list[str]:
    found = []
    if BLOCKED_NAME.search(path):
        found.append("file type that only comes from the game or a build")
    if len(data) > MAX_BYTES:
        found.append(f"larger than {MAX_BYTES // (1024 * 1024)} MB")
    head = data[:64]
    if head.startswith(b"UnityFS\0") or head.startswith(b"UnityWeb") or is_unity_serialized(data):
        found.append("Unity asset data")
    if b"FSB5" in head[:8] or head.startswith(b"BKHD") or head.startswith(b"RIFF") and b"WAVE" in head[:16] and path.endswith(".wem"):
        found.append("sound bank")
    if any(head.startswith(m) for m in GAME_MAGIC):
        found.append("the game's save or data file")
    if any(m.search(data) for m in DECOMPILED_MARKERS):
        found.append("looks like decompiled game code")
    return found


def check(staged: bool, commit: str | None = None) -> list[str]:
    where = f"{commit[:7]} " if commit else ""
    bad = []
    for mode, path in files_to_check(staged, commit):
        if mode == "120000":
            bad.append(f"  {where}{path}: symlink (git mode 120000); commit the file itself, not a link")
            continue
        try:
            data = read(path, staged, commit)
        except (OSError, subprocess.CalledProcessError):
            continue
        bad += [f"  {where}{path}: {p}" for p in problems_in(path, data)]
    return bad


def main() -> int:
    args = sys.argv[1:]
    staged = "--staged" in args
    bad = []
    if "--commits" in args:
        for commit in args[args.index("--commits") + 1:]:
            bad += check(False, commit)
    else:
        bad = check(staged)
    if bad:
        print("Blocked: these files must not be in the repo.", file=sys.stderr)
        print("\n".join(bad), file=sys.stderr)
        print("Game data stays on the player's PC; the importer reads it at runtime.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
