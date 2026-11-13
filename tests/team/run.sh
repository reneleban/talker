#!/bin/sh
# Führt alle Team-Tests aus (make team-test). Braucht nur jq (Fake-hc, kein Server, kein Claude).
# Läuft nicht in CI, nicht im pre-commit und nicht in `make verify`.
cd "$(dirname "$0")" || exit 1
rc=0
for t in test_*.sh check_docs.sh; do
  [ -f "$t" ] || continue
  echo "== $t"
  sh "$t" || rc=1
done
exit $rc
