# Security Policy

talker verarbeitet sensible Daten: Mikrofon-Audio, transkribierten Text und
das Nutzer-Clipboard, und besitzt die macOS-Accessibility-Permission
(Tastatur-Events). Sicherheitslücken in diesen Bereichen nehmen wir ernst.

## Lücke melden — bitte privat

**Bitte keine öffentlichen GitHub-Issues für Sicherheitslücken.**

Bevorzugter Weg: **GitHub Security Advisory** — im Repo unter
*Security → Report a vulnerability* (private Meldung an die Maintainer).

Alternativ per E-Mail an **rene@leban.de** (Betreff mit `[SECURITY]` beginnen).

Hilfreich in der Meldung: betroffene Version/Commit, Reproduktionsschritte,
erwartetes vs. tatsächliches Verhalten, Einschätzung der Auswirkung
(z. B. Audio-/Clipboard-Leak, Rechteausweitung über Accessibility).

## Was du erwarten kannst

Solo-maintained, Best-Effort:

- **Eingangsbestätigung innerhalb von 7 Tagen.**
- Einschätzung + geplanter Fix-Weg, sobald reproduziert.
- Koordinierte Veröffentlichung: bitte 90 Tage Zeit für einen Fix, bevor
  Details öffentlich werden.
- Namensnennung in den Release Notes, wenn gewünscht.

## Unterstützte Versionen

Es wird nur die jeweils **neueste Version** mit Fixes versorgt (Projekt in
früher Entwicklungsphase, siehe `docs/stability.md`).
