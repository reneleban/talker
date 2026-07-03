# ADR-0007 — Source-first-Distribution ohne Notarisierung

- Status: akzeptiert
- Datum: 2026-07-03
- Kontext: siehe `CONTEXT.md` und README (Abschnitte „Source-first", „App-Bundle & Permissions")
- Baut auf: ADR-0002 (self-contained, eine Toolchain — das Prinzip zieht sich bis zur Distribution durch)

## Kontext und Problemstellung

talker braucht zwei macOS-Permissions: Bedienungshilfen (globaler Hotkey + Text-Injection) und Mikrofon. macOS bindet erteilte Permissions an die Code-Signatur der App — jede neue Signatur macht daraus systemseitig eine „neue" App, und alle Rechte sind weg. Zu entscheiden ist der Distributionsweg: ein signiertes + notarisiertes Bundle über ein Apple Developer Program, oder eine source-first-Verteilung, bei der jeder Nutzer selbst aus dem Quellcode baut. Diese Entscheidung legt Zielgruppe, laufende Kosten, Onboarding-Aufwand und Update-Mechanismus fest und ist teuer zu revidieren, weil sie Marketing-Positionierung und Build-/Signatur-Pipeline gleichzeitig betrifft.

## Entscheidungstreiber

- Privacy-first/Open-Source-Kernaussage: talker lauscht am Mikrofon und verarbeitet Diktate — Vertrauen ist das zentrale Verkaufsargument.
- Solo-Projekt ohne Umsatzmodell — laufende Kosten und Prozess-Overhead müssen zum Aufwand passen.
- Primäre Zielgruppe sind ohnehin Entwickler/AI-Coding-CLI-Nutzer (siehe Voice-to-Prompt).
- self-contained-Prinzip aus ADR-0001/0002 — möglichst wenige externe Abhängigkeiten und Runtimes.
- Das Permission-an-Signatur-Binding von macOS muss lokal beherrschbar bleiben.

## Betrachtete Optionen

1. **Signiertes + notarisiertes Bundle** über ein Apple Developer Program (99 USD/Jahr). Glatte Installation ohne Gatekeeper-Warnung, ermöglicht DMG-/Homebrew-Cask-Distribution mit fertigen Binaries und potenziell Auto-Update (z.B. Sparkle).
2. **Source-first**: kein fertiges Binary wird verteilt. Jeder Nutzer baut talker selbst (`make install` als Einzeiler). Signatur lokal ad-hoc bzw. selbstsigniert — `make cert` legt einmalig ein Codesign-Zertifikat `talker-dev` im Login-Schlüsselbund an; danach bleiben Bundle-ID + Zertifikat stabil, und Updates verlieren die Rechte nicht mehr.

## Entscheidung

**Option 2: source-first.** Kein Apple-Developer-Account, keine Notarisierung, kein signiertes Bundle zur Verteilung. Nutzer bauen aus dem Quellcode (`make install`); das Permission-Verlust-Problem wird lokal über `make cert` (stabiles selbstsigniertes Zertifikat) gelöst, ohne Apple-Account.

## Begründung

- Passt zur privacy-first/Open-Source-Kernaussage: „jede Zeile auditierbar, MIT-lizenziert, kein Blackbox-Binary" wird als Trust-Feature positioniert — kein Bequemlichkeits-Nachteil, sondern das eigentliche Verkaufsargument für eine App, die am Mikrofon lauscht.
- Vermeidet laufende Kosten (99 USD/Jahr) und den Notarisierungs-Workflow-Aufwand — für ein Solo-Projekt ohne Umsatzmodell nicht gerechtfertigt.
- `make cert` löst das macOS-Permission-an-Signatur-Problem lokal und ohne Apple-Account: Bundle-ID + selbstsigniertes Zertifikat bleiben stabil, Updates behalten die Rechte.
- Der Build ist bewusst schlank/reproduzierbar gehalten (ein `make install`-Einzeiler), um die source-first-Hürde so klein wie möglich zu halten.

## Konsequenzen

- **Positiv:** volle Auditierbarkeit als Trust-Feature; keine laufenden Kosten, kein Notarisierungs-Prozess; self-contained bis in die Distribution; `make cert` macht Permissions über Updates hinweg stabil.
- **Negativ (explizit akzeptiert):**
  - Höhere Einstiegshürde — Rust-Toolchain + Build-Zeit statt Doppelklick-Installer. Zielgruppe faktisch auf technisch versierte Nutzer/Entwickler eingeschränkt; deckt sich mit der ohnehin gewählten primären Zielgruppe.
  - Keine DMG- oder Homebrew-Cask-Distribution mit fertigen Binaries; Homebrew wäre nur als „build-from-source"-Formel denkbar.
  - Kein Auto-Update-Mechanismus (z.B. Sparkle) möglich — dafür wäre eine vertrauenswürdige Signatur-Kette nötig.
  - macOS-Gatekeeper-Warnung bei der ersten Ausführung (ad-hoc- bzw. lokal selbstsignierte App) — dokumentierter Workaround statt eleganter Lösung.
  - Das Known-Issue „Permissions nach Update weg" ist eine direkte Folge dieser Entscheidung. `make cert` mildert es ab (stabiles lokales Zertifikat), eliminiert es aber nicht: die Rechte müssen einmalig pro Mac nach dem ersten `make cert`-Signaturwechsel neu erteilt werden.
- **Verworfen:** Apple Developer Program + Notarisierung — Kosten und Prozessaufwand für ein Solo-Projekt nicht gerechtfertigt; widerspricht zudem der bewussten Entscheidung, aktuell keinen breiten Launch/keine breite Distribution zu verfolgen.
