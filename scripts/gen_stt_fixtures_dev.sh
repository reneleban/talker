#!/bin/bash
# Entwickler-Alltagssätze als STT-Fixtures, gesprochen von Microsoft-Neural-
# Stimmen (edge-tts, braucht Netz). Ergänzt die say-Basis-Fixtures
# (scripts/gen_stt_fixtures.sh). Namenspräfix: dev-.
set -euo pipefail
cd "$(dirname "$0")/.."
command -v edge-tts >/dev/null || { echo "edge-tts fehlt (pip/uv install edge-tts)"; exit 1; }
DEST=tests/fixtures/stt
mkdir -p "$DEST"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# name|voice|text — de-Stimmen sprechen die englischen Fachbegriffe (Akzent-Fall).
CASES=$(cat <<'EOF'
dev-java|de-DE-ConradNeural|Die Spring Boot Anwendung wirft eine NullPointerException im UserService.
dev-typescript|de-DE-KatjaNeural|Ich habe das Interface in TypeScript um ein optionales Feld erweitert und den Type Guard angepasst.
dev-node|de-DE-ConradNeural|Der Node Server crasht beim npm install wegen einer kaputten package lock.
dev-css|de-DE-KatjaNeural|Setz das Flexbox Layout auf space between und gib dem Container mehr Padding.
dev-html|de-DE-ConradNeural|Das Formular braucht noch ein Label und ein Input Feld vom Typ E-Mail.
dev-docker|de-DE-KatjaNeural|Bau das Docker Image neu und push es in die Registry, sonst startet der Container nicht.
dev-rust|de-DE-ConradNeural|Der Borrow Checker meckert, weil die Lifetime der Referenz zu kurz ist.
dev-llm|de-DE-KatjaNeural|Der System Prompt ist zu lang, das Kontextfenster des LLM läuft über.
dev-git|de-DE-ConradNeural|Rebase deinen Feature Branch auf main und mach danach einen Force Push.
dev-kubernetes|de-DE-KatjaNeural|Das Deployment in Kubernetes hängt, weil das Pod Limit erreicht ist.
dev-react|de-DE-ConradNeural|Der useEffect Hook feuert doppelt, weil die Dependency Liste falsch ist.
dev-api|de-DE-KatjaNeural|Die REST API liefert einen 404, weil die Route nicht registriert ist.
dev-anweisung-branch|de-DE-ConradNeural|Leg einen neuen Branch an, implementiere die Validierung im Config-Modul und schreib einen Test dazu.
dev-anweisung-css|de-DE-KatjaNeural|Ändere die Hintergrundfarbe des Buttons auf blau und erhöhe das Padding auf zwölf Pixel.
dev-anweisung-refactor|de-DE-ConradNeural|Refaktoriere den UserService, extrahiere die Datenbankzugriffe in ein Repository und aktualisiere die Tests.
dev-anweisung-docker|de-DE-KatjaNeural|Füge dem Dockerfile einen Healthcheck hinzu und begrenze den Speicher des Containers auf fünfhundert Megabyte.
EOF
)

while IFS='|' read -r name voice text; do
  [ -z "$name" ] && continue
  edge-tts --voice "$voice" --text "$text" --write-media "$TMP/$name.mp3" >/dev/null 2>&1
  afconvert -f WAVE -d LEI16@16000 -c 1 "$TMP/$name.mp3" "$DEST/$name.wav"
  printf '%s' "$text" > "$DEST/$name.ref.txt"
  echo "✓ $name ($voice)"
done <<< "$CASES"
echo "Fixtures: $DEST"
