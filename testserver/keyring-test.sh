#!/bin/bash
# Read a keyring-3-shaped Secret Service item with keyring 4.
# Uses a private D-Bus session and a throwaway login keyring so the test
# never touches a real user's passwords or SQLCipher key.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
export XDG_DATA_HOME="$TMP/data"
export XDG_CONFIG_HOME="$TMP/config"
export XDG_CACHE_HOME="$TMP/cache"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-$TMP/run}"
mkdir -p "$XDG_DATA_HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
cd "$ROOT"
dbus-run-session -- bash -c '
  set -euo pipefail
  # Create and unlock a login keyring. The password is only for this session.
  gnome-keyring-daemon --components=secrets --daemonize --unlock <<< "foobar"
  # Give the daemon a moment to claim the well-known name.
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    if busctl --user list 2>/dev/null | grep -q org.freedesktop.secrets; then
      break
    fi
    sleep 0.3
  done
  exec env NEUTRON_KEYRING=1 cargo test -p neutron-core --test keyring_compat -- --nocapture
'
