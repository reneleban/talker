#!/bin/sh
# T7: spawn.sh — Validierung und HERDR_ENV (AK-11). Alle Fälle ohne echten Agenten; „vor jeder Änderung"
# heißt: Worktree-Liste, Branches und Panes sind danach unverändert.
. "$(dirname "$0")/lib.sh"

th_begin
th_server

SP='sh .claude/skills/spawn-worker/spawn.sh'
snapshot() {
  git -C "$TH_REPO" worktree list --porcelain
  git -C "$TH_REPO" branch --list
  th_h pane list | jq -c '[.result.panes[].pane_id]'
  th_h agent list | jq -c '[.result.agents[].name]'
}
# check <name> <erwarteter Exit> <teilstring der Meldung> <befehl>
check() {
  before=$(snapshot)
  inpane "$4"
  assert_eq "$1: Exit $2" "$2" "$INPANE_RC"
  [ -z "$3" ] || assert_contains "$1: Meldung" "$3" "$INPANE_OUT"
  assert_eq "$1: nichts geändert" "$before" "$(snapshot)"
}

# --- Integrations-Worktree fehlt (noch kein init) ---
check "AK-11 Integrations-Worktree fehlt" 1 "init" "$SP rust-developer 0001 chore/0001-x"

inpane "$SP init 0001 chore/0001-x"
assert_eq "init: Exit 0" 0 "$INPANE_RC"

# --- Aufruf/Optionen → Exit 2 ---
check "AK-11 ohne Argumente" 2 "Aufruf" "$SP"
check "AK-11 zu wenige Argumente" 2 "Aufruf" "$SP rust-developer 0001"
check "AK-11 zu viele Argumente" 2 "Aufruf" "$SP rust-developer 0001 chore/0001-x extra"
check "AK-11 unbekannte Option" 2 "--bogus" "$SP --bogus rust-developer 0001 chore/0001-x"
check "AK-11 Option ohne Rolle" 2 "Aufruf" "$SP --instance"

# --- außerhalb von herdr ---
check "AK-11 HERDR_ENV leer" 1 "make dev" "HERDR_ENV= $SP rust-developer 0001 chore/0001-x"
check "AK-11 HERDR_ENV=0" 1 "make dev" "HERDR_ENV=0 $SP rust-developer 0001 chore/0001-x"
check "AK-11 close außerhalb von herdr" 1 "make dev" "HERDR_ENV= $SP close rust-developer-0001"
inpane "HERDR_ENV= $SP init 0002 chore/0002-x"
assert_eq "AK-11 init braucht herdr nicht" 0 "$INPANE_RC"

# --- Ticket vierstellig ---
check "AK-11 Ticket dreistellig" 1 "vierstellig" "$SP rust-developer 043 chore/0001-x"
check "AK-11 Ticket fünfstellig" 1 "vierstellig" "$SP rust-developer 00431 chore/0001-x"
check "AK-11 Ticket nicht numerisch" 1 "vierstellig" "$SP rust-developer abcd chore/0001-x"

# --- Rolle bekannt ---
check "AK-11 unbekannte Rolle" 1 "unbekannte Rolle" "$SP kein-agent 0001 chore/0001-x"

# --- --instance nur für Rollen mit eigenem Worktree ---
check "AK-7 --instance bei tech-lead" 1 "--instance" "$SP --instance tech-lead 0001 chore/0001-x"

# --- schmutziger Ziel-Worktree ---
git -C "$TH_REPO" worktree add -q -B wt/rust-developer "$TH_WT/rust-developer" chore/0001-x
echo dreck >"$TH_WT/rust-developer/unversioniert.txt"
check "AK-11 schmutziger Ziel-Worktree" 1 "uncommittete" "$SP rust-developer 0001 chore/0001-x"
assert_eq "AK-11 schmutziger Worktree bleibt unangetastet" "?? unversioniert.txt" "$(git -C "$TH_WT/rust-developer" status --porcelain)"

th_summary
