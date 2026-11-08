#!/bin/bash
# Lädt 15 fest gewählte deutsche Clips aus FLEURS (google/fleurs, CC-BY-4.0) und legt sie als
# tests/fixtures/stt/fleurs-de-NN.wav (16 kHz mono, 16 bit) + fleurs-de-NN.ref.txt ab.
# 01..10 (dev, männliche Sprecher), 11..15 (train, Sprecherinnen; Geschlecht aus der Spalte `gender`
# der .tsv, das Skript bricht ab, wenn sie nicht FEMALE ist). Der Download passiert nur hier (nie im App-Binary). Die Clips sind committet; das Skript
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

# nn|Split|Clip-ID im FLEURS-Split (de_de, Dateiname ohne .wav)
CLIPS=$(cat <<'LIST'
01|dev|2451196745503996684
02|dev|10249467818655484680
03|dev|11948867539067866220
04|dev|6269851957822964868
05|dev|14464692325699699874
06|dev|5149639213376445783
07|dev|13482481987250861856
08|dev|13429363889456608330
09|dev|9168806669820613156
10|dev|9835644362854829850
11|train|13757244091402192321
12|train|12352489473809577518
13|train|17726102859095899
14|train|8317008729752988728
15|train|8048032612956319554
LIST
)
# Clips, die laut .tsv (Spalte 7, gender) weiblich sein müssen.
FEMALE_MIN=11

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$DEST"

for split in dev train; do
  mkdir -p "$TMP/$split"
  echo "Lade $split.tsv und $split.tar.gz (dev ca. 220 MB, train ca. 1,6 GB) …"
  curl -fsSL --proto '=https' --proto-redir '=https' "$BASE/$split.tsv" -o "$TMP/$split.tsv"
  curl -fsSL --proto '=https' --proto-redir '=https' "$BASE/audio/$split.tar.gz" -o "$TMP/$split.tar.gz"
  MEMBERS=()
  while IFS='|' read -r nn sp id; do
    [ "$sp" = "$split" ] && MEMBERS+=("$split/$id.wav")
  done <<< "$CLIPS"
  tar -xzf "$TMP/$split.tar.gz" -C "$TMP" "${MEMBERS[@]}"
  rm -f "$TMP/$split.tar.gz"
done

while IFS='|' read -r nn split id; do
  [ -z "$nn" ] && continue
  if [ "$((10#$nn))" -ge "$FEMALE_MIN" ]; then
    gender=$(awk -F'\t' -v f="$id.wav" '$2 == f { print $7 }' "$TMP/$split.tsv")
    [ "$gender" = "FEMALE" ] || { echo "Clip $id ist laut $split.tsv nicht FEMALE ($gender)" >&2; exit 1; }
  fi
  afconvert -f WAVE -d LEI16@16000 -c 1 "$TMP/$split/$id.wav" "$DEST/fleurs-de-$nn.wav"
  # Referenz: Spalte 3 (raw_transcription), ohne Zeilenende wie bei den say-Fixtures.
  awk -F'\t' -v f="$id.wav" '$2 == f { printf "%s", $3 }' "$TMP/$split.tsv" > "$DEST/fleurs-de-$nn.ref.txt"
  [ -s "$DEST/fleurs-de-$nn.ref.txt" ] || { echo "Keine Referenz für $id" >&2; exit 1; }
  echo "✓ fleurs-de-$nn ($split/$id)"
done <<< "$CLIPS"
# Die Liste muss alle erzeugten Dateien abdecken, sonst bliebe ein Clip ungeprüft.
for f in "$DEST"/fleurs-de-*.wav "$DEST"/fleurs-de-*.ref.txt; do
  grep -q "  $(basename "$f")\$" "$DEST/fleurs-de.sha256" || { echo "Nicht in fleurs-de.sha256: $f" >&2; exit 1; }
done
# Gegen die eingecheckte Liste prüfen (schlägt bei veränderten Download-Inhalten fehl).
(cd "$DEST" && shasum -a 256 -c fleurs-de.sha256)
echo "Fixtures: $DEST/fleurs-de-*"
