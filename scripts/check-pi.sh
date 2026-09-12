#!/usr/bin/env bash
# Smoke-test a deployed server over the LAN. Usage: scripts/check-pi.sh [host:port]
set -euo pipefail

BASE="${1:-192.168.1.246:3000}"
URL="http://${BASE}"
pass=0; fail=0
ok() { echo "PASS  $1"; pass=$((pass+1)); }
no() { echo "FAIL  $1"; fail=$((fail+1)); }

code=$(curl -s -m 5 -o /tmp/health.json -w "%{http_code}" "$URL/api/health")
[ "$code" = 200 ] && grep -q '"status":"ok"' /tmp/health.json && ok "health 200 {status:ok}" || no "health ($code)"

for path in / /assets/style.css /assets/app.js; do
    code=$(curl -s -m 5 -o /dev/null -w "%{http_code}" "$URL$path")
    [ "$code" = 200 ] && ok "static $path 200" || no "static $path ($code)"
done

NEW=$(curl -s -m 5 -X POST "$URL/api/game/new" -H 'Content-Type: application/json' -d '{}')
CID=$(printf '%s' "$NEW" | sed -n 's/.*"card_id": *"\([0-9a-f]\{32\}\)".*/\1/p')
[ -n "$CID" ] && ok "new game -> card_id" || { no "new game"; exit 1; }

REV=$(curl -s -m 5 -X POST "$URL/api/game/reveal" -H 'Content-Type: application/json' \
    -d "{\"card_id\":\"$CID\",\"cell_id\":4}")
printf '%s' "$REV" | grep -q '"cell_id":4' && printf '%s' "$REV" | grep -q '"symbol"' \
    && ok "reveal cell 4" || no "reveal: $REV"

code=$(curl -s -m 5 -o /dev/null -w "%{http_code}" -X POST "$URL/api/game/reveal" \
    -H 'Content-Type: application/json' -d "{\"card_id\":\"$CID\",\"cell_id\":4}")
[ "$code" = 409 ] && ok "duplicate reveal -> 409" || no "duplicate reveal ($code)"

code=$(curl -s -m 5 -o /dev/null -w "%{http_code}" -X POST "$URL/api/game/reveal" \
    -H 'Content-Type: application/json' -d "{\"card_id\":\"$CID\",\"cell_id\":9}")
[ "$code" = 400 ] && ok "invalid cell -> 400" || no "invalid cell ($code)"

FIN=$(curl -s -m 5 -X POST "$URL/api/game/finish" -H 'Content-Type: application/json' \
    -d "{\"card_id\":\"$CID\"}")
printf '%s' "$FIN" | grep -qE '"win":(true|false)' && printf '%s' "$FIN" | grep -q '"message"' \
    && ok "finish -> $FIN" || no "finish: $FIN"

code=$(curl -s -m 5 -o /dev/null -w "%{http_code}" -X POST "$URL/api/game/finish" \
    -H 'Content-Type: application/json' -d "{\"card_id\":\"$CID\"}")
[ "$code" = 409 ] && ok "double finish -> 409" || no "double finish ($code)"

code=$(curl -s -m 5 -o /dev/null -w "%{http_code}" "$URL/api/game/finish" -X POST \
    -H 'Content-Type: application/json' -d '{}')
[ "$code" = 400 ] && ok "empty card_id -> 400" || no "empty card_id ($code)"

echo ""
echo "$pass passed, $fail failed"
[ "$fail" = 0 ]
