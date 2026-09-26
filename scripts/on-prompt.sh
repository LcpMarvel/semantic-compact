#!/bin/sh
# UserPromptSubmit entry point: make sure the binary exists, then exec it.
# Provisioning normally happens once right after install (or after an
# update); afterwards this is a plain `test -x` plus exec. stdin/stdout/exit
# code pass straight through, so the fail-open contract is preserved even
# when provisioning fails — the binary just never runs and we exit 0.
set -u

root="$(cd "$(dirname "$0")/.." && pwd)"
bin="$root/bin/semantic-compact"

if [ ! -x "$bin" ]; then
  sh "$root/scripts/setup.sh" >/dev/null 2>&1
fi

if [ -x "$bin" ]; then
  exec "$bin"
fi

exit 0
