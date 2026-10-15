#!/bin/bash
# Run the live evolution-data-server contact tests in a private D-Bus session
# with a throwaway address book. Requires evolution-data-server.
# The XDG variables are exported BEFORE dbus-run-session on purpose: the EDS
# services are D-Bus-activated and inherit the bus daemon's environment.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
export XDG_DATA_HOME="$TMP/data" XDG_CONFIG_HOME="$TMP/config" XDG_CACHE_HOME="$TMP/cache"
cd "$ROOT"
dbus-run-session -- env NEUTRON_EDS=1 cargo test -p neutron-core --test eds_contacts -- --nocapture
# And the graceful-degradation path: no session bus at all.
DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent NEUTRON_EDS_ABSENT=1 \
  cargo test -p neutron-core --test eds_contacts -- --nocapture
