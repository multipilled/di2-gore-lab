#!/usr/bin/env python3
"""Fails if any of the given commits carries attribution that must not reach GitHub.

Run by .githooks/pre-push on the commits being pushed (never on older history). Rejects:
  - a Cursor Agent author or committer (name "Cursor Agent" or a cursoragent@ email)
  - an author or committer email that isn't a users.noreply.github.com address
Commit-message trailers (Co-authored-by and the like) are allowed (the owner, 2026-10-10, 9:21 PM).

Usage:
  python tools/check_commit_attribution.py SHA...
"""
from __future__ import annotations

import re
import subprocess
import sys

NOREPLY = "@users.noreply.github.com"
CURSOR_NAME = re.compile(r"cursor[\s_-]*agent", re.IGNORECASE)


def git(*args: str) -> str:
    return subprocess.run(["git", *args], check=True, capture_output=True).stdout.decode("utf-8", "replace")


def problems_in(commit: str) -> list[str]:
    an, ae, cn, ce = git("show", "-s", "--format=%an%x00%ae%x00%cn%x00%ce", commit).rstrip("\n").split("\0", 3)
    found = []
    for role, name, email in (("author", an, ae), ("committer", cn, ce)):
        if CURSOR_NAME.search(name) or "cursoragent@" in email.lower():
            found.append(f"{role} is Cursor Agent ({name} <{email}>)")
        elif not email.lower().endswith(NOREPLY):
            found.append(f"{role} email <{email}> is not a {NOREPLY[1:]} address")
    return found


def main() -> int:
    bad = []
    for commit in sys.argv[1:]:
        bad += [f"  {commit[:7]}: {p}" for p in problems_in(commit)]
    if bad:
        print("Blocked: these commits carry attribution that must not be pushed.", file=sys.stderr)
        print("\n".join(bad), file=sys.stderr)
        print(f"Set user.name/user.email to the {NOREPLY[1:]} identity,", file=sys.stderr)
        print("then fix each commit, e.g. `git commit --amend --no-edit --reset-author`.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
