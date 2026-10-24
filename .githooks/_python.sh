#!/bin/sh
# Shared by the hooks (sourced, not run): sets PY to a Python 3.8+ that actually starts.
# Tries python3, then python, then `py -3`. Each is run once, because on Windows the
# Microsoft Store python3/python stubs exist on PATH but fail or hang instead of running.
find_python() {
  if command -v timeout >/dev/null 2>&1; then _t="timeout 10"; else _t=""; fi
  for _cand in "python3" "python" "py -3"; do
    # shellcheck disable=SC2086
    set -- $_cand
    command -v "$1" >/dev/null 2>&1 || continue
    if $_t "$@" -c "import sys; sys.exit(0 if sys.version_info >= (3, 8) else 1)" >/dev/null 2>&1; then
      PY="$_cand"
      return 0
    fi
  done
  echo "$HOOK_NAME: no working Python 3.8+ found (tried python3, python, py -3)." >&2
  echo "$HOOK_NAME: refusing to continue, because the game-file guard could not run." >&2
  exit 1
}
