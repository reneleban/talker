# Stabilitäts-Policies — talker

Stabilität als Vertrag („Apache-artig"): keine Governance-Bürokratie, nur die
Engineering-Disziplin. Diese drei Policies gelten für jeden Change im Repo.

## 1. SemVer

talker versioniert nach [Semantic Versioning 2.0.0](https://semver.org/lang/de/)
(`MAJOR.MINOR.PATCH`, Quelle: `version` in `Cargo.toml`).

- **MAJOR**: Breaking Change (Definition unten).
- **MINOR**: neues Feature, abwärtskompatibel (neuer Cleanup-Modus, neue
  Config-Option mit Default, neue UI-Fähigkeit).
- **PATCH**: Bugfix/Verhaltenskorrektur ohne neues Feature.

Vor 1.0.0 gilt SemVer-üblich: Breaking Changes erhöhen MINOR (0.x.y → 0.(x+1).0),
werden aber im CHANGELOG genauso explizit als **Breaking** markiert.

### Was ist ein Breaking Change?

- **Config-Format**: ein bestehendes `config.toml`-Feld wird entfernt, umbenannt,
  ändert Typ oder Bedeutung — oder eine alte Datei lädt nicht mehr zu identischem
  Verhalten. Das TOML-Format ist Teil des öffentlichen Vertrags (Policy 3).
- **Verhalten**: dokumentiertes Nutzerverhalten ändert sich (z.B. Default-Hotkey,
  Injection-Strategie, Fallback-Semantik „Cleanup-Fehler → Raw Transcript").
- **Systemanforderungen**: höhere macOS-Mindestversion, neue Pflicht-Permission,
  anderes/zusätzliches Pflicht-Modell auf der Platte.
- **CLI/Prozess-Schnittstelle**: falls künftig vorhanden (Flags, Exit-Codes).

Kein Breaking Change: interne Refactors, Log-Texte, Latenz-Verbesserungen,
Austausch einer Engine hinter `Transcriber`/`LlmCleaner` bei gleichem Verhalten.

## 2. CHANGELOG

`CHANGELOG.md` im Repo-Root, Format [Keep a Changelog 1.1.0](https://keepachangelog.com/de/1.1.0/):
Sektionen `Added`/`Changed`/`Deprecated`/`Removed`/`Fixed`/`Security`,
neueste Version oben, `[Unreleased]` sammelt bis zum Release.

- Jede nutzersichtbare Änderung bekommt einen Eintrag im selben PR.
- Breaking Changes werden mit **Breaking:** geflaggt und nennen den Migrationspfad.
- Rein interne Änderungen (Refactor, Tests, CI) brauchen keinen Eintrag.

## 3. Config-Versionierung & Migration

Das TOML-Config-Format (`~/Library/Application Support/talker/config.toml`)
ist ein **Kompatibilitäts-Vertrag**: jede jemals von talker geschriebene Config
muss von allen späteren Versionen derselben MAJOR-Linie ladbar bleiben.

Regeln:

- **Zwei Stellen pro Feld**: jedes neue Feld berührt in `src/config.rs`
  `Config` (mit Doc-Kommentar) und `impl Default for Config`. Struct-weites
  `#[serde(default)]` füllt fehlende Felder aus dem Default — ein vergessenes
  Feld ist damit ein Compile-Fehler, kein stiller Reset (Tests decken die
  Abwärtskompatibilität ab).
- **Neue Felder sind optional**: immer mit sinnvollem Default — eine alte Datei
  ohne das Feld lädt unverändert (`#[serde(default)]`).
- **Deprecation statt stillem Break**: ein Feld wird nie einfach entfernt oder
  umgedeutet. Stattdessen: neues Feld einführen, das alte in der
  Legacy-Migration von `Config::parse()` auf das neue abbilden, im CHANGELOG
  unter `Deprecated` ankündigen. Entfernen des Legacy-Pfads erst mit der
  nächsten MAJOR-Version.
  **Referenzbeispiel**: `cleanup_enabled` (bool) → `cleanup_mode` (Enum) —
  `parse()` migriert das Legacy-Bool vor dem Deserialisieren (`false` → `raw`,
  `true` → `business`), explizites `cleanup_mode` gewinnt, falscher Typ
  bleibt ein Fehler.
- **Falscher Typ ist ein Fehler, kein stiller Fallback**: Typfehler in einem
  bekannten Feld führen zum Parse-Fehler → `load()` meldet den Hinweis und
  nutzt Defaults, überschreibt die Datei aber nicht ungefragt.
- **Unbekannte Felder** werden toleriert (Vorwärtskompatibilität beim Downgrade),
  gehen beim nächsten `save()` aber verloren.
