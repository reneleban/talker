# ADR-0006 — MIT-Lizenz für den Code (Modelle bleiben unter eigenen Terms)

- Status: akzeptiert
- Datum: 2026-07-03
- Kontext: siehe `docs/licensing.md`
- Umgesetzt: `LICENSE` (MIT), `license = "MIT"` in `Cargo.toml` (Copyright © 2026 René Leban)

## Kontext und Problemstellung

Vor der Open-Source-Veröffentlichung (github.com/reneleban/talker) musste eine Lizenz für den Quellcode gewählt werden. In der engeren Auswahl standen die beiden verbreitetsten permissiven OSS-Lizenzen: **MIT** und **Apache-2.0**. Die Wahl ist nachträglich zwar änderbar, aber sozial teuer (Contributor-Erwartung, Header-Policy, `deny.toml`-Gate) — sie sollte einmal bewusst getroffen und dokumentiert werden.

Wichtige Abgrenzung: An anderer Stelle wurde für talker von „Apache-artiger Stabilität" gesprochen. Das bezieht sich **ausdrücklich nicht auf die Lizenz**, sondern auf das Engineering-Reifegrad-Ziel eines gereiften Apache-Software-Foundation-Projekts (kompromisslose Tests, Stabilitäts-Verträge/SemVer, statische Qualität, Robustheit, Review-Disziplin — dokumentiert in `docs/stability.md` und `CLAUDE.md`, unabhängig von der Lizenzwahl). Lizenz und Qualitätsziel sind bewusst entkoppelt; dieses ADR entscheidet allein die Code-Lizenz.

## Entscheidungstreiber

- Minimale rechtliche Hürde für Nutzer und Contributor eines Solo-Hobby-Projekts.
- Kompatibilität mit allen Abhängigkeiten (kein technischer Lizenz-Zwang durchsetzen).
- Einfachheit/Textlänge — Pflege-Rauschen niedrig halten (siehe SPDX-Header-Policy in `docs/licensing.md`).
- Klare Trennung zwischen Code-Lizenz und den davon abweichenden Modell-Terms.

## Betrachtete Optionen

1. **MIT.** Kürzeste, am wenigsten restriktive verbreitete OSS-Lizenz.
2. **Apache-2.0.** Permissiv mit explizitem Patent-Grant und Contributor-Klauseln.

## Entscheidung

**Option 1: MIT-Lizenz für den gesamten Quellcode** (`LICENSE` im Root, Copyright © 2026 René Leban; `license = "MIT"` in `Cargo.toml`). Die von talker genutzten ML-Modelle sind davon ausgenommen und unterliegen ihren **eigenen Terms** — sie werden nicht im Repo mitgebundelt, sondern vom Nutzer selbst mit Lizenz-Zustimmung bezogen (ADR-0003, `docs/licensing.md`).

## Begründung

- **Kein technischer Zwang zu einer bestimmten Lizenz durch Dependencies.** llama.cpp (MIT), sherpa-onnx (Apache-2.0), egui/cpal/objc2 (MIT/Apache-2.0-Dual) — sowohl MIT als auch Apache-2.0 wären mit allen Abhängigkeiten kompatibel gewesen. Die Entscheidung war damit frei und rein präferenzgetrieben.
- **MIT ist die einfachste, am wenigsten restriktive Option** — minimale rechtliche Hürde für Nutzer und Contributor, kürzester Text.
- **Apache-2.0s Mehrwert greift hier nicht.** Der explizite Patent-Grant und die Contributor-Klauseln adressieren primär Firmen-Contributor und Firmen-Patentrisiko. Für ein Solo-Hobby-Projekt ohne solches Risiko stehen sie in keinem sinnvollen Verhältnis zur zusätzlichen Textlänge/Komplexität — daher zugunsten der MIT-Einfachheit verworfen.
- **Der lizenzrechtlich kritische Teil liegt ohnehin nicht im Code, sondern in den Modellen.** Die genutzten ML-Modelle unterliegen eigenen, nicht-MIT/Apache-kompatiblen Terms: gemma4:e2b (Google Gemma Terms of Use — kein OSS) und Parakeet TDT 0.6b v3 (CC-BY-4.0, Namensnennung NVIDIA; laut Model Card geprüft 2026-07-03). Sie werden **nicht im Repo mitgebundelt**, sondern vom Nutzer selbst mit Zustimmung heruntergeladen. Damit redistribuiert talker nichts und die Modell-Pflichten liegen beim Nutzer. Die klare Trennung **Code = MIT, Modelle = eigene Terms** (bereits in `docs/licensing.md` dokumentiert) ist der Grund, warum die Code-Lizenzwahl allein (MIT vs Apache) rechtlich unkritisch bleibt.

## Konsequenzen

- **Positiv:** minimale rechtliche Hürde für Contributor/Nutzer; kürzester, klarster Lizenztext; keine Header-Pflege (SPDX-Header-Policy in `docs/licensing.md`); volle Kompatibilität mit allen aktuellen Abhängigkeiten.
- **Negativ:** kein expliziter Patent-Grant wie bei Apache-2.0. Für den aktuellen Scope (Solo-Projekt, kein Firmen-Patentrisiko) ohne praktische Folgen; sollte das Projekt je Firmen-Contributor gewinnen, ist diese Entscheidung erneut abzuwägen.
- **Folge-Aktionen (bereits verankert):** `cargo deny check` (`deny.toml`) prüft neue Dependencies weiterhin auf Lizenz-Kompatibilität gegen MIT als CI-Gate; `THIRD-PARTY.html` (via `cargo about`) dokumentiert die Abhängigkeits-Lizenzen und wird bei Dependency-Änderungen neu erzeugt. Ein künftiger App-interner Modell-Downloader muss die jeweiligen Modell-Terms anzeigen und Zustimmung einholen, bevor geladen wird.
- **Verworfen:** Apache-2.0 (unnötige Komplexität/Textlänge für dieses Projekt; Patent-/Contributor-Klauseln ohne praktischen Mehrwert bei diesem Scope, s.o.).
