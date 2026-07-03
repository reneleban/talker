# talker — Brand-Palette & Logo (Ticket-0024)

Solider Platzhalter, kein Studio-Logo: konsistent zu App-Icon
(`examples/gen_icon.rs`) und Siri-Overlay-Palette (Ticket-0007). Ein
professionell gestaltetes Logo kann später extern entstehen und diese
Assets 1:1 ersetzen.

## Assets

| Datei | Einsatz |
|---|---|
| `resources/brand/talker-logo-light.svg` | helle Hintergründe (Wortmarke Indigo) |
| `resources/brand/talker-logo-dark.svg` | dunkle Hintergründe (Wortmarke Weiß) |

Einbindung im README (folgt dem GitHub-Farbschema automatisch):

```html
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="resources/brand/talker-logo-dark.svg">
  <img src="resources/brand/talker-logo-light.svg" alt="talker" width="300">
</picture>
```

Die Bildmarke (Squircle + Mikrofon) nutzt exakt die Glyph-Geometrie des
App-Icons (44er-Raster aus `examples/gen_icon.rs`/`src/tray.rs`) — kein
visueller Bruch zwischen Dock-Icon, Menüleiste und Repo.

## Palette

Abgeleitet aus der Overlay-Wave-Palette (`DEFAULT_OVERLAY_COLORS`,
`src/config.rs`) und dem Icon-Verlauf (`examples/gen_icon.rs`):

| Farbe | Hex | Quelle / Einsatz |
|---|---|---|
| Indigo (dunkel) | `#262454` | Icon-Verlauf oben; Wortmarke auf hell |
| Nachtblau | `#181630` | Icon-Verlauf unten; dunkle Flächen |
| Eisweiß | `#F2F7FF` | Overlay-Wave 1; Wortmarke auf dunkel |
| Cyan | `#59D9FF` | Overlay-Wave 2; Akzent-Verlauf Start |
| Blau | `#4073FF` | Overlay-Wave 3; Akzent-Verlauf Mitte |
| Violett | `#9E66FF` | Overlay-Wave 4; Akzent-Verlauf Ende |
| Glyph-Weiß | `#F5F5F5` | Mikrofon-Glyph in Icon/Logo |

Akzent-Verlauf (Unterstreichung im Logo): Cyan → Blau → Violett,
horizontal — dieselbe Anmutung wie die Overlay-Leuchtspuren.

## Typografie

Wortmarke: System-Sans (SF Pro Display bzw. Fallback Segoe UI/Helvetica),
Gewicht 600, leicht negative Laufweite. Bewusst als Font-Stack im SVG
(kein eingebetteter Font, keine Pfad-Konvertierung) — für einen
Repo-Platzhalter ausreichend; bei einem echten Logo später Text in Pfade
wandeln.
