---
name: test-first
description: Test-first-Vorgehen für talker (red-green-refactor) mit AK-zu-Test-Zuordnung und Grenzfällen. Nutzen, wenn Verhalten implementiert oder ein Bug behoben wird.
---

# test-first

## Zyklus
1. **Rot**: Ein Test, der genau ein Stück Verhalten aus einer AK ausdrückt.
   Laufen lassen, er muss aus dem erwarteten Grund fehlschlagen (nicht wegen
   Tippfehler oder fehlendem Import).
2. **Grün**: minimaler Code, bis der Test besteht. Keine Vorgriffe auf spätere Tests.
3. **Refactor**: Duplikate und Namen aufräumen, Tests bleiben grün.

Bug-Fix: erst ein Test, der den Bug reproduziert (rot), dann der Fix.

## talker-Konventionen
- Unit-Tests im `#[cfg(test)] mod tests` der Datei; Integrationstests in `tests/`.
- Testkommentar nennt die Anforderung: `/// REQ-STREAM-009: …` (siehe `src/config.rs`).
- Testnamen beschreiben Verhalten: `fehlender_insert_mode_faellt_auf_am_ende_zurueck`.
- Tests prüfen Verhalten über die öffentliche Schnittstelle des Moduls, nicht
  interne Details.
- Echte Hardware/OS-APIs (Mikrofon, CGEventTap, Clipboard, AppKit) hinter die
  vorhandenen Nähte stellen und dort mit Fakes testen; den Rest deckt der
  Field-Test vor dem Release ab.
- Tests, die Modelle brauchen, nur in `tests/` (laufen in `make verify`, nicht
  in CI und nicht im pre-commit).

## Pflicht je Story
- Jede AK → mind. ein Test (1:1 nachvollziehbar).
- Grenzfälle: leer, Min/Max, ein-über/-unter-Grenze, Fehlerpfad, Abbruch.
- Out-of-Scope als „darf nicht passieren"-Test, wo sinnvoll.
- Voll abgedeckt: Clipboard save/restore, Resampling, TOML load/save und
  Migration, Cleanup-Fallback.
