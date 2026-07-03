# CONTEXT — talker

Glossar der kanonischen Begriffe für **talker**, einen lokal laufenden Diktier-Assistenten für macOS — eine quelloffene, voll on-device laufende Alternative zu Cloud-Dictation-Diensten.

Dieses Dokument ist **nur ein Glossar** — keine Implementierungsdetails, keine Spec. Entscheidungen stehen in `docs/adr/`.

## Begriffe

- **Dictation** — der gesamte Vorgang: Nutzer spricht, talker wandelt Sprache in bereinigten Text um und fügt ihn in die aktive App ein.
- **Utterance** — eine einzelne zusammenhängende Spracheingabe zwischen Drücken und Loslassen der Push-to-talk-Taste. In v1 exakt eine Aufnahme.
- **Push-to-talk (PTT)** — Interaktionsmodell: eine globale Taste wird gehalten, solange gesprochen wird; das Loslassen beendet die Utterance. (Alternative Modi wie Toggle/Hands-free sind spätere Features.)
- **Raw Transcript** — der unbearbeitete Text direkt aus der Spracherkennung, vor jeder Bereinigung.
- **Cleaned Transcript** — der bereinigte Text nach dem Cleanup-Schritt (Füllwörter entfernt, Interpunktion, Formatierung). Das ist, was eingefügt wird, sofern Cleanup aktiv ist.
- **Cleanup** — der Schritt, der den Raw Transcript zu einem Cleaned Transcript veredelt. Ausfallsicher: fällt er aus, wird der Raw Transcript eingefügt.
- **Cleanup-Modus** — das aktive Stil-Profil, das bestimmt, WIE der Cleanup bereinigt. Jeder Modus ist ein benanntes Prompt-Profil an dasselbe LLM. Modi: `Roh` (kein LLM, ersetzt den früheren Cleanup-aus-Schalter), `Geschäftlich` (formal, entfernt auch unsichere Floskeln wie „glaube ich"), `Natürlich` (Stil erhalten: nur Füllwörter/Interpunktion korrigieren, Ton und Wortwahl des Sprechers bleiben — locker wie förmlich), `LLM-optimiert` (diktierte Rede wird zu einem klaren, strukturierten Prompt für eine AI-Coding-CLI wie Claude Code aufbereitet: eindeutige Anweisung, Rambling/Füllsel raus, sinnvolle Struktur). Genau ein Modus ist zur Zeit aktiv.
- **Injection** — das Einfügen des fertigen Textes in die aktuell fokussierte Ziel-App.
- **Target App** — die App, die zum Zeitpunkt der Injection den Fokus hat und den Text empfängt.
- **Batch-Modell** — Verhalten in v1: Text wird komplett erst nach dem Loslassen eingefügt (nicht live während des Sprechens). Live-Streaming ist ein späteres Feature.
