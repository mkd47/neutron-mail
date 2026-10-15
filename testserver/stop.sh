#!/usr/bin/env bash
BASE="${NM_TEST_BASE:-/workspace/testmail}"
HERE="$(cd "$(dirname "$0")" && pwd)"
[[ -f "$BASE/run/master.pid" ]] && kill "$(cat "$BASE/run/master.pid")" 2>/dev/null
pkill -f "smtp_server.py" 2>/dev/null || true
pkill -f "http.server 10880" 2>/dev/null || true
echo stopped
