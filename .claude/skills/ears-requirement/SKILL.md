---
name: ears-requirement
description: Fachliche Anforderungen für talker im EARS-Format schreiben und unter docs/requirements/ ablegen. Nutzen, wenn eine Spec Anforderungen festlegt oder ändert.
---

# ears-requirement

Grundlage: `docs/requirements/README.md` (Muster, Ablage) und ADR-0004.

## Muster (ein Satz = eine Anforderung = mind. ein Test)
| Muster | Schablone |
|---|---|
| Ubiquitär | „Das System soll `<Verhalten>`." |
| Event | „WENN `<Trigger>`, soll das System `<Verhalten>`." |
| State | „WÄHREND `<Zustand>`, soll das System `<Verhalten>`." |
| Optional | „WO `<Feature>` aktiv ist, soll das System `<Verhalten>`." |
| Unerwünscht | „FALLS `<Fehlerfall>`, DANN soll das System `<Verhalten>`." |

## Regeln
- ID `REQ-<BEREICH>-<NNN>`, fortlaufend je Datei; bestehende IDs nie umnummerieren
  oder wiederverwenden. Entfallene Anforderung: Zeile behalten, mit
  „(entfallen, Ticket NNNN)" markieren.
- Eine Datei je Feature-Bereich (siehe README); neuer Bereich → neue Datei
  im Stil von `streaming-dictation.md` und in der README-Liste ergänzen.
- Begriffe exakt wie in `CONTEXT.md`; neuer Begriff → dort ergänzen.
- Prüfbar formulieren: messbare Werte (ms, Anzahl, Zustand), kein „schnell",
  „benutzerfreundlich", „sollte möglichst".
- Jeder Fehlerpfad aus der Spec bekommt eine „FALLS … DANN"-Anforderung.
- Config-Felder: Default und Verhalten bei fehlendem Feld als eigene Anforderung.

## Selbstprüfung
Für jede Anforderung kannst du in einem Satz sagen, welcher Test sie prüft.
Wenn nicht: umformulieren oder teilen.
