# Fachliche Anforderungen — EARS statt IREB-Fließtext

Format-Entscheidung: [ADR-0004](../adr/adr-0004-doku-format-ears-und-adr-diagramme.md).
Begriffe: [`CONTEXT.md`](../../CONTEXT.md). Architektur: [`docs/architecture/overview.md`](../architecture/overview.md).

## Die fünf EARS-Muster

Jede Anforderung ist **ein** Satz, **eine** Testbarkeits-Einheit (deckt sich
mit der AK↔Test-1:1-Pflicht aus `CLAUDE.md`). Muster nach Anforderungstyp:

| Muster | Schablone | Wann |
|---|---|---|
| Ubiquitär | „Das System soll `<Verhalten>`." | Immer gültig, kein Trigger/Zustand |
| Event-getrieben | „WENN `<Trigger>`, soll das System `<Verhalten>`." | Reaktion auf ein Ereignis |
| State-getrieben | „WÄHREND `<Zustand>`, soll das System `<Verhalten>`." | Verhalten gilt nur in einem Zustand |
| Optionales Feature | „WO `<Feature>` aktiv ist, soll das System `<Verhalten>`." | Feature-Flag/Config-abhängig |
| Unerwünschtes Verhalten | „FALLS `<Fehlerfall>`, DANN soll das System `<Verhalten>`." | Fehlerpfade, Grenzfälle |

Komplexe Anforderungen kombinieren mehrere Muster in einem Satz (z. B.
State + Event). Jede Anforderung bekommt eine ID `REQ-<Bereich>-<NNN>` für
Rückverweise aus Tests/Tickets.

## Ablage-Konvention

Eine Datei pro Feature-Bereich, benannt nach dem Bereich (nicht nach
Ticket-Nummern — Tickets sind Umsetzungs-Historie, diese Dateien sind der
aktuelle Soll-Zustand):

- `dictation-core.md` — PTT, Audio-Capture, STT, Injection, Overlay
- `cleanup-modes.md` — Cleanup-Modi, Vokabular-Korrektur
- `onboarding-permissions.md` — Settings, Erst-Start, Permissions, Login-Item
- `context-awareness.md` — Per-App-Modus-Auflösung
- `model-downloader.md` — Modell-Download, Lizenz-Consent, Checksum, Resume

Bei neuen Features: erst die EARS-Sätze hier ergänzen, dann Tests schreiben,
dann implementieren (TDD-Reihenfolge aus `CLAUDE.md`).
