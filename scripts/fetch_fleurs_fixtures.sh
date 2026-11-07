#!/bin/bash
# Lädt 10 fest gewählte deutsche Clips aus FLEURS (google/fleurs, CC-BY-4.0) und legt sie als
# tests/fixtures/stt/fleurs-de-NN.wav (16 kHz mono, 16 bit) + fleurs-de-NN.ref.txt ab.
# Der Download passiert nur hier (nie im App-Binary). Die Clips sind committet; das Skript
# muss nur laufen, wenn man sie neu erzeugen will. Deterministisch: feste Datensatz-Revision,
# feste Clip-IDs, ein zweiter Lauf liefert byte-identische Dateien
# (am Ende geprüft gegen tests/fixtures/stt/fleurs-de.sha256).
# Lizenz und Urheber: tests/fixtures/stt/ATTRIBUTION.md. Braucht Netz, curl, tar, afconvert (macOS).
set -euo pipefail
cd "$(dirname "$0")/.."
DEST=tests/fixtures/stt
# Fest gepinnte Revision des Datensatzes auf Hugging Face.
REV=70bb2e84b976b7e960aa89f1c648e09c59f894dd
BASE="https://huggingface.co/datasets/google/fleurs/resolve/$REV/data/de_de"

# nn|Clip-ID im FLEURS-Split dev (de_de, Dateiname ohne .wav)
CLIPS=$(cat <<'LIST'
01|2451196745503996684
02|10249467818655484680
03|11948867539067866220
04|6269851957822964868
05|14464692325699699874
06|5149639213376445783
07|13482481987250861856
08|13429363889456608330
09|9168806669820613156
10|9835644362854829850
LIST
)

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$DEST"

echo "Lade dev.tsv und dev.tar.gz (ca. 220 MB) …"
curl -fsSL --proto '=https' --proto-redir '=https' "$BASE/dev.tsv" -o "$TMP/dev.tsv"
curl -fsSL --proto '=https' --proto-redir '=https' "$BASE/audio/dev.tar.gz" -o "$TMP/dev.tar.gz"

MEMBERS=()
while IFS='|' read -r nn id; do
  [ -z "$nn" ] && continue
  MEMBERS+=("dev/$id.wav")
done <<< "$CLIPS"
tar -xzf "$TMP/dev.tar.gz" -C "$TMP" "${MEMBERS[@]}"

while IFS='|' read -r nn id; do
  [ -z "$nn" ] && continue
  afconvert -f WAVE -d LEI16@16000 -c 1 "$TMP/dev/$id.wav" "$DEST/fleurs-de-$nn.wav"
  # Referenz: Spalte 3 (raw_transcription), ohne Zeilenende wie bei den say-Fixtures.
  awk -F'\t' -v f="$id.wav" '$2 == f { printf "%s", $3 }' "$TMP/dev.tsv" > "$DEST/fleurs-de-$nn.ref.txt"
  [ -s "$DEST/fleurs-de-$nn.ref.txt" ] || { echo "Keine Referenz für $id" >&2; exit 1; }
  echo "✓ fleurs-de-$nn ($id)"
done <<< "$CLIPS"
# Gegen die eingecheckte Liste prüfen (schlägt bei veränderten Download-Inhalten fehl).
(cd "$DEST" && shasum -a 256 -c fleurs-de.sha256)
echo "Fixtures: $DEST/fleurs-de-*"
