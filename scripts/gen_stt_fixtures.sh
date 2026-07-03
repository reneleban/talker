#!/bin/bash
# Erzeugt synthetische STT-Test-Fixtures (macOS `say`, 16 kHz mono WAV + .ref.txt).
# Deterministisch; echte Aufnahmen können zusätzlich als <name>.wav/<name>.ref.txt
# in tests/fixtures/stt/ gelegt werden (Präfix "real-" empfohlen).
set -euo pipefail
cd "$(dirname "$0")/.."
DEST=tests/fixtures/stt
mkdir -p "$DEST"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# name|voice|text  — Anna (de) spricht auch die de/en-Mixe (Akzent-Fall).
CASES=$(cat <<'EOF'
de-business|Anna|Wir verschieben das Meeting auf Freitag, weil der Kunde erst dann Zeit hat.
de-zahlen|Anna|Der Termin ist am dritten März um vierzehn Uhr dreißig und das Budget beträgt zwölftausendfünfhundert Euro.
en-plain|Samantha|Please make sure the login button also works with the enter key.
mix-claude-cli|Anna|Öffne die Claude CLI und starte den Befehl noch einmal.
mix-feature-flag|Anna|Wir sollten das Feature Flag für den Dark Mode erst nach dem Code Review mergen.
mix-tech|Anna|Der Commit ist gepusht, aber die Pipeline hängt beim Cargo Build.
mix-tools|Anna|Ich habe das Overlay mit egui gebaut und die Config als TOML gespeichert.
de-frage|Anna|Hast du schon mit dem Kunden über den Wartungsvertrag gesprochen?
EOF
)

while IFS='|' read -r name voice text; do
  [ -z "$name" ] && continue
  say -v "$voice" "$text" -o "$TMP/$name.aiff"
  afconvert -f WAVE -d LEI16@16000 -c 1 "$TMP/$name.aiff" "$DEST/$name.wav"
  printf '%s' "$text" > "$DEST/$name.ref.txt"
  echo "✓ $name ($voice)"
done <<< "$CASES"
echo "Fixtures: $DEST"
