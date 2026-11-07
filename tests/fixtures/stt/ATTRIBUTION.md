# Attribution — FLEURS-Fixtures (`fleurs-de-*.wav`, `fleurs-de-*.ref.txt`)

Die zehn Dateien `fleurs-de-01` … `fleurs-de-10` stammen **nicht** aus diesem Repo-Code (MIT),
sondern aus dem Datensatz FLEURS und stehen unter **CC-BY-4.0**. Die übrigen Fixtures in
diesem Ordner (`de-*`, `dev-*`, `en-*`, `mix-*`) sind mit macOS `say` erzeugt und gehören dazu nicht.

## Angaben nach CC BY 4.0

| | |
|---|---|
| **Urheber** | Alexis Conneau, Min Ma, Simran Khanuja, Yu Zhang, Vera Axelrod, Siddharth Dalmia, Jason Riesa, Clara Rivera, Ankur Bapna (Google) |
| **Titel** | FLEURS: Few-shot Learning Evaluation of Universal Representations of Speech (Datensatz „FLEURS“, Konfiguration `de_de`, Split `dev`) |
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

Zehn Clips, fest per Clip-ID im Skript `scripts/fetch_fleurs_fixtures.sh` gewählt (3 bis 12 s,
ohne Ziffern im Satz, jeder Satz nur einmal). Der Split `de_de/dev` enthält ausschließlich
männliche Sprecher.

| Fixture | Clip-ID (`dev`, `de_de`) |
|---|---|
| `fleurs-de-01` | 2451196745503996684 |
| `fleurs-de-02` | 10249467818655484680 |
| `fleurs-de-03` | 11948867539067866220 |
| `fleurs-de-04` | 6269851957822964868 |
| `fleurs-de-05` | 14464692325699699874 |
| `fleurs-de-06` | 5149639213376445783 |
| `fleurs-de-07` | 13482481987250861856 |
| `fleurs-de-08` | 13429363889456608330 |
| `fleurs-de-09` | 9168806669820613156 |
| `fleurs-de-10` | 9835644362854829850 |

## Änderungen gegenüber dem Original

- Audio: aus dem Archiv `dev.tar.gz` entnommen und mit `afconvert` in WAV (16-bit PCM,
  16 kHz, mono) umgewandelt; sonst unverändert (kein Schnitt, keine Filterung).
- Referenztext (`.ref.txt`): Spalte `raw_transcription` aus `dev.tsv`, unverändert, ohne Zeilenende.
- Umbenennung der Dateien (`fleurs-de-NN` statt Clip-ID).

Neu erzeugen: `scripts/fetch_fleurs_fixtures.sh` (Netz nötig; zweiter Lauf liefert identische Dateien).
