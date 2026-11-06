---
name: field-test
description: Abnahme von talker vor einem Release — App bauen, installieren und die AKs der enthaltenen Tickets mit dem Menschen real durchspielen. Nutzen nur im Release-Schritt.
---

# field-test

Den Field-Test macht der **Mensch** an seinem Mac. Du bereitest vor, führst
durch und protokollierst; du klickst nicht selbst und meldest nichts als
bestanden, was der Mensch nicht bestätigt hat.

## Vorbereitung
1. Enthaltene Tickets: `CHANGELOG.md` `[Unreleased]` und `git log <letzter-tag>..main`.
2. Checkliste aus deren AKs bauen, nur nutzersichtbares Verhalten, je Punkt:
   Schritt → erwartetes Ergebnis.
3. Immer dabei (Regression der Kernpipeline):
   - PTT halten, sprechen, loslassen → Text erscheint im fokussierten Feld.
   - Clipboard-Inhalt vor dem Diktat ist danach unverändert.
   - Jeder Cleanup-Modus einmal; Live-Diktat (Satzweise) einmal.
   - App-Neustart mit bestehender `config.toml` → Einstellungen erhalten.

## Durchführung
1. Dem Menschen sagen: `make install` (baut, signiert mit `talker-dev`,
   ersetzt das installierte Bundle). Fehlt das Zertifikat: einmalig `make cert`.
2. **Stopp-Punkt Permissions**: Fragt macOS nach Mikrofon oder Bedienungshilfen,
   kann nur der Mensch sie erteilen. Erklären, wo (Systemeinstellungen →
   Datenschutz & Sicherheit), und warten.
3. Checkliste Punkt für Punkt abfragen (AskUserQuestion: bestanden / nicht
   bestanden / übersprungen, mit Notiz).
4. Bei „nicht bestanden": Log-Auszug erbitten
   (`log show --last 5m --predicate 'process == "talker"'`), Befund notieren.

## Ergebnis
Protokoll (Datum, Version-Kandidat, Punkte mit Ergebnis, Befunde). Ein nicht
bestandener Punkt blockiert das Release; Befunde werden neue Tickets.
