//! Zustandslogik des Aufnahme-Indikators + Pegel→Waveform-Mapping.
//! Bewusst fensterfrei (unit-testbar); das Rendering lebt in `overlay`.

use std::time::{Duration, Instant};

use crate::audio::LevelHandle;

/// Wie lange done/error sichtbar bleiben, bevor das Overlay verschwindet.
const DONE_SHOW: Duration = Duration::from_millis(700);
const ERROR_SHOW: Duration = Duration::from_millis(2500);
/// Glättung des Pegels pro UI-Frame (Anteil des neuen Werts).
const LEVEL_ATTACK: f32 = 0.35;
const LEVEL_DECAY: f32 = 0.12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Hidden,
    /// Modelle werden beim App-Start geladen — kein Timeout, endet via `ready()`.
    Loading,
    /// Kurzes „bereit"-Feedback nach dem Laden.
    Ready,
    /// Live-Vorschau aus den Einstellungen (synthetischer Pegel, kein Mikro).
    Preview,
    Recording,
    Transcribing,
    Done,
    Error(String),
}

pub struct Indicator {
    phase: Phase,
    phase_since: Instant,
    level: Option<LevelHandle>,
    smoothed_level: f32,
}

impl Default for Indicator {
    fn default() -> Self {
        Self {
            phase: Phase::Hidden,
            phase_since: Instant::now(),
            level: None,
            smoothed_level: 0.0,
        }
    }
}

impl Indicator {
    pub fn loading(&mut self, now: Instant) {
        self.phase = Phase::Loading;
        self.phase_since = now;
    }

    /// Modelle geladen → kurzes „bereit", dann automatisch ausblenden.
    pub fn ready(&mut self, now: Instant) {
        if self.phase == Phase::Loading {
            self.phase = Phase::Ready;
            self.phase_since = now;
        }
    }

    /// Vorschau ein/aus (aus den Einstellungen). Ein echtes Diktat hat Vorrang
    /// und wird nicht überschrieben; „aus" beendet nur die Vorschau selbst.
    pub fn set_preview(&mut self, on: bool, now: Instant) {
        if on && matches!(self.phase, Phase::Hidden | Phase::Done | Phase::Ready) {
            self.phase = Phase::Preview;
            self.phase_since = now;
        } else if !on && self.phase == Phase::Preview {
            self.cancel();
        }
    }

    pub fn start_recording(&mut self, now: Instant, level: LevelHandle) {
        self.phase = Phase::Recording;
        self.phase_since = now;
        self.level = Some(level);
        self.smoothed_level = 0.0;
    }

    pub fn transcribing(&mut self, now: Instant) {
        self.phase = Phase::Transcribing;
        self.phase_since = now;
        self.level = None;
    }

    pub fn finish_ok(&mut self, now: Instant) {
        self.phase = Phase::Done;
        self.phase_since = now;
    }

    pub fn fail(&mut self, now: Instant, msg: impl Into<String>) {
        self.phase = Phase::Error(msg.into());
        self.phase_since = now;
    }

    /// Aufnahme kam nicht zustande / wurde verworfen → sofort verstecken.
    pub fn cancel(&mut self) {
        self.phase = Phase::Hidden;
        self.level = None;
    }

    /// Zeitgesteuerte Übergänge (done/error blenden automatisch aus).
    pub fn tick(&mut self, now: Instant) {
        let timeout = match self.phase {
            Phase::Done | Phase::Ready => DONE_SHOW,
            Phase::Error(_) => ERROR_SHOW,
            _ => return,
        };
        if now.duration_since(self.phase_since) >= timeout {
            self.cancel();
        }
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    pub fn visible(&self) -> bool {
        self.phase != Phase::Hidden
    }

    /// Liest den Live-Pegel und glättet ihn (schneller Attack, langsamer Decay
    /// — die Waveform „atmet" statt zu zappeln). Aufruf einmal pro UI-Frame.
    /// `gain` hebt das Mikrofon-RMS auf 0..1 an (typisch 10, konfigurierbar).
    pub fn smoothed_level(&mut self, gain: f32) -> f32 {
        let raw = self.level.as_ref().map_or(0.0, LevelHandle::get);
        // Typische Sprech-RMS liegen um 0.02–0.2 → kräftig auf 0..1 anheben.
        let target = (raw * gain).clamp(0.0, 1.0);
        let k = if target > self.smoothed_level {
            LEVEL_ATTACK
        } else {
            LEVEL_DECAY
        };
        self.smoothed_level += (target - self.smoothed_level) * k;
        self.smoothed_level
    }
}

/// Parameter der überlagerten Wellen-Layer (Frequenz, Laufgeschwindigkeit,
/// Phasenversatz, Amplituden-Gewicht) — gegenläufig für den „lebendigen" Look.
pub const WAVE_LAYERS: [(f32, f32, f32, f32); 4] = [
    (1.8, 4.1, 0.0, 1.0),
    (2.6, -5.6, 1.7, 0.75),
    (3.4, 7.4, 3.9, 0.55),
    (1.3, -3.1, 5.1, 0.9),
];

/// Sampelt einen Wellen-Layer: `n` y-Werte in −1..1 über die Breite (x 0..1).
/// Ränder laufen auf 0 aus (Siri-Look); bei Stille bleibt eine ruhige Idle-Welle.
pub fn wave_samples(level: f32, t: f32, layer: usize, n: usize) -> Vec<f32> {
    let (freq, speed, phase, weight) = WAVE_LAYERS[layer % WAVE_LAYERS.len()];
    let amp = (0.15 + 1.05 * level.clamp(0.0, 1.0)) * weight;
    (0..n)
        .map(|i| {
            let x = i as f32 / (n.max(2) - 1) as f32;
            let envelope = (std::f32::consts::PI * x).sin().powi(2);
            let carrier = (x * freq * std::f32::consts::TAU + t * speed + phase).sin();
            // Zweite, langsame Modulation, damit die Welle „atmet" statt rotiert.
            let breath = 0.75 + 0.25 * (t * 1.3 + phase).sin();
            (amp * envelope * carrier * breath).clamp(-1.0, 1.0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Instant {
        Instant::now()
    }

    #[test]
    fn full_lifecycle_recording_to_done_hides_after_timeout() {
        let t0 = now();
        let mut ind = Indicator::default();
        assert!(!ind.visible());

        ind.start_recording(t0, LevelHandle::default());
        assert_eq!(*ind.phase(), Phase::Recording);
        assert!(ind.visible());

        ind.transcribing(t0);
        assert_eq!(*ind.phase(), Phase::Transcribing);

        ind.finish_ok(t0);
        assert_eq!(*ind.phase(), Phase::Done);

        ind.tick(t0 + Duration::from_millis(100));
        assert_eq!(*ind.phase(), Phase::Done, "zu früh versteckt");
        ind.tick(t0 + Duration::from_millis(800));
        assert!(!ind.visible(), "Done muss nach Timeout ausblenden");
    }

    #[test]
    fn error_is_shown_longer_than_done() {
        let t0 = now();
        let mut ind = Indicator::default();
        ind.start_recording(t0, LevelHandle::default());
        ind.transcribing(t0);
        ind.fail(t0, "STT fehlgeschlagen");
        assert_eq!(*ind.phase(), Phase::Error("STT fehlgeschlagen".into()));

        ind.tick(t0 + Duration::from_millis(1000));
        assert!(ind.visible(), "Error muss länger stehen als Done");
        ind.tick(t0 + Duration::from_millis(2600));
        assert!(!ind.visible());
    }

    #[test]
    fn loading_stays_until_ready_then_auto_hides() {
        let t0 = now();
        let mut ind = Indicator::default();
        ind.loading(t0);
        assert_eq!(*ind.phase(), Phase::Loading);

        // Kein Timeout während des Ladens.
        ind.tick(t0 + Duration::from_secs(30));
        assert_eq!(*ind.phase(), Phase::Loading);

        ind.ready(t0 + Duration::from_secs(30));
        assert_eq!(*ind.phase(), Phase::Ready);
        ind.tick(t0 + Duration::from_secs(31));
        assert!(!ind.visible(), "Ready muss automatisch ausblenden");
    }

    #[test]
    fn ready_is_ignored_outside_loading() {
        let t0 = now();
        let mut ind = Indicator::default();
        ind.start_recording(t0, LevelHandle::default());
        ind.ready(t0);
        assert_eq!(
            *ind.phase(),
            Phase::Recording,
            "ready darf Recording nicht stören"
        );
    }

    #[test]
    fn ptt_during_loading_switches_to_recording() {
        let t0 = now();
        let mut ind = Indicator::default();
        ind.loading(t0);
        ind.start_recording(t0, LevelHandle::default());
        assert_eq!(*ind.phase(), Phase::Recording);
    }

    #[test]
    fn preview_toggles_but_never_overrides_recording() {
        let t0 = now();
        let mut ind = Indicator::default();

        ind.set_preview(true, t0);
        assert_eq!(*ind.phase(), Phase::Preview);
        assert!(ind.visible());
        ind.tick(t0 + Duration::from_secs(60));
        assert_eq!(*ind.phase(), Phase::Preview, "Preview hat kein Timeout");

        // Echtes Diktat übernimmt; Preview-Aus beendet Recording NICHT.
        ind.start_recording(t0, LevelHandle::default());
        ind.set_preview(false, t0);
        assert_eq!(*ind.phase(), Phase::Recording);
        // Preview-An während Recording wird ignoriert.
        ind.set_preview(true, t0);
        assert_eq!(*ind.phase(), Phase::Recording);

        ind.cancel();
        ind.set_preview(true, t0);
        assert_eq!(*ind.phase(), Phase::Preview);
        ind.set_preview(false, t0);
        assert!(!ind.visible());
    }

    #[test]
    fn cancel_hides_immediately_from_any_phase() {
        let t0 = now();
        let mut ind = Indicator::default();
        ind.start_recording(t0, LevelHandle::default());
        ind.cancel();
        assert!(!ind.visible());
    }

    #[test]
    fn tick_does_nothing_while_recording_or_transcribing() {
        let t0 = now();
        let mut ind = Indicator::default();
        ind.start_recording(t0, LevelHandle::default());
        ind.tick(t0 + Duration::from_secs(60));
        assert_eq!(*ind.phase(), Phase::Recording);
    }

    #[test]
    fn smoothed_level_rises_fast_and_decays_slow() {
        let mut ind = Indicator::default();
        let h = LevelHandle::default();
        ind.start_recording(now(), h.clone());

        h.set(0.2); // lautes Sprechen
        let mut rising = Vec::new();
        for _ in 0..5 {
            rising.push(ind.smoothed_level(10.0));
        }
        assert!(
            rising.windows(2).all(|w| w[1] >= w[0]),
            "muss steigen: {rising:?}"
        );
        let peak = *rising.last().unwrap();
        assert!(peak > 0.5, "Pegel muss deutlich anheben: {peak}");

        h.set(0.0); // Stille → langsamer Abfall, nicht sofort 0
        let after_one = ind.smoothed_level(10.0);
        assert!(
            after_one > peak * 0.7,
            "Decay zu schnell: {after_one} vs {peak}"
        );
    }

    #[test]
    fn wave_idle_calm_speech_strong_and_bounded() {
        let n = 60;
        for layer in 0..WAVE_LAYERS.len() {
            let mut idle_max = 0.0f32;
            let mut loud_max = 0.0f32;
            for t in [0.0f32, 0.3, 0.9, 1.7, 2.6, 4.1] {
                let idle = wave_samples(0.0, t, layer, n);
                let loud = wave_samples(1.0, t, layer, n);
                assert_eq!(idle.len(), n);
                assert!(
                    loud.iter().all(|y| (-1.0..=1.0).contains(y)),
                    "Grenzen verletzt"
                );
                // Ränder laufen aus (Envelope) — Float-Toleranz.
                assert!(idle[0].abs() < 1e-6, "linker Rand: {}", idle[0]);
                assert!(loud[n - 1].abs() < 1e-6, "rechter Rand: {}", loud[n - 1]);
                idle_max = idle_max.max(idle.iter().fold(0.0f32, |m, y| m.max(y.abs())));
                loud_max = loud_max.max(loud.iter().fold(0.0f32, |m, y| m.max(y.abs())));
            }
            assert!(idle_max <= 0.2, "Idle-Layer {layer} zu wild: {idle_max}");
            assert!(
                loud_max > idle_max * 2.0,
                "Layer {layer} reagiert nicht auf Pegel: idle {idle_max} loud {loud_max}"
            );
        }
    }

    #[test]
    fn wave_handles_degenerate_sizes_and_layer_wraps() {
        assert!(wave_samples(0.5, 1.0, 0, 0).is_empty());
        assert_eq!(wave_samples(0.5, 1.0, 7, 1).len(), 1);
    }
}
