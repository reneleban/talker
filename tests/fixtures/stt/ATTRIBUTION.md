# Attribution — FLEURS-Fixtures (`fleurs-de-*.wav`, `fleurs-de-*.ref.txt`)

Die fünfzehn Dateien `fleurs-de-01` … `fleurs-de-15` stammen **nicht** aus diesem Repo-Code (MIT),
sondern aus dem Datensatz FLEURS und stehen unter **CC-BY-4.0**. Die übrigen Fixtures in
diesem Ordner (`de-*`, `dev-*`, `en-*`, `mix-*`) sind mit macOS `say` erzeugt und gehören dazu nicht.

## Angaben nach CC BY 4.0

| | |
|---|---|
| **Urheber** | Alexis Conneau, Min Ma, Simran Khanuja, Yu Zhang, Vera Axelrod, Siddharth Dalmia, Jason Riesa, Clara Rivera, Ankur Bapna (Google) |
| **Titel** | FLEURS: Few-shot Learning Evaluation of Universal Representations of Speech (Datensatz „FLEURS“, Konfiguration `de_de`, Splits `dev` und `train`) |
| **Quelle Datensatz** | <https://huggingface.co/datasets/google/fleurs>, fest gepinnte Revision `70bb2e84b976b7e960aa89f1c648e09c59f894dd` |
| **Quelle Paper** | <https://arxiv.org/abs/2205.12446> |
| **Lizenz** | CC-BY-4.0 — Creative Commons Attribution 4.0 International: <https://creativecommons.org/licenses/by/4.0/> |

Wörtlich gegen die Datensatz-Seite geprüft (2026-11-07, `README.md` der Revision oben):

- Metadaten: `license: cc-by-4.0`
- Fließtext: „All datasets are licensed under the [Creative Commons license (CC-BY)](https://creativecommons.org/licenses/).“
- Zitierwunsch: „Please cite the paper when referencing the FLEURS corpus“ — Paper: arXiv:2205.12446.

Der Fließtext verlinkt nur die allgemeine Lizenzübersicht; die konkrete Version 4.0 steht in den
Metadaten (`cc-by-4.0`). Der Link oben zeigt auf den Lizenztext dieser Version.

Die Datensatz-Seite nennt keinen Gewährleistungsausschluss für die Daten; die Lizenz CC BY 4.0
schließt in § 5 jede Gewährleistung aus („as-is“).

## Auswahl

Fünfzehn Clips, fest per Clip-ID im Skript `scripts/fetch_fleurs_fixtures.sh` gewählt (3 bis 12 s,
ohne Ziffern im Satz, jeder Satz nur einmal, kein Satz in zwei Splits). Der Split `de_de/dev` enthält
ausschließlich männliche Sprecher (ebenso `test`); die Clips 11 bis 15 stammen deshalb aus dem Split
`train` und sind Sprecherinnen. Das Geschlecht steht in Spalte 7 (`gender`) der jeweiligen `.tsv`;
das Skript bricht ab, wenn sie bei einem dieser Clips nicht `FEMALE` ist.

| Fixture | Split | Clip-ID (`de_de`) | Geschlecht (`gender`) |
|---|---|---|---|
| `fleurs-de-01` | `dev` | 2451196745503996684 | MALE |
| `fleurs-de-02` | `dev` | 10249467818655484680 | MALE |
| `fleurs-de-03` | `dev` | 11948867539067866220 | MALE |
| `fleurs-de-04` | `dev` | 6269851957822964868 | MALE |
| `fleurs-de-05` | `dev` | 14464692325699699874 | MALE |
| `fleurs-de-06` | `dev` | 5149639213376445783 | MALE |
| `fleurs-de-07` | `dev` | 13482481987250861856 | MALE |
| `fleurs-de-08` | `dev` | 13429363889456608330 | MALE |
| `fleurs-de-09` | `dev` | 9168806669820613156 | MALE |
| `fleurs-de-10` | `dev` | 9835644362854829850 | MALE |
| `fleurs-de-11` | `train` | 13757244091402192321 | FEMALE |
| `fleurs-de-12` | `train` | 12352489473809577518 | FEMALE |
| `fleurs-de-13` | `train` | 17726102859095899 | FEMALE |
| `fleurs-de-14` | `train` | 8317008729752988728 | FEMALE |
| `fleurs-de-15` | `train` | 8048032612956319554 | FEMALE |

## Änderungen gegenüber dem Original

- Audio: aus dem Archiv `dev.tar.gz` bzw. `train.tar.gz` entnommen und mit `afconvert` in WAV (16-bit PCM,
  16 kHz, mono) umgewandelt; sonst unverändert (kein Schnitt, keine Filterung).
- Referenztext (`.ref.txt`): Spalte `raw_transcription` aus der `.tsv` des Splits, unverändert, ohne Zeilenende.
- Umbenennung der Dateien (`fleurs-de-NN` statt Clip-ID).

Neu erzeugen: `scripts/fetch_fleurs_fixtures.sh` (Netz nötig; zweiter Lauf liefert identische Dateien).
