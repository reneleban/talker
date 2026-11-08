//! Zustandslogik des Aufnahme-Indikators + Pegel→Waveform-Mapping.
//! Bewusst fensterfrei (unit-testbar); das Rendering lebt in `overlay`.

use std::time::{Duration, Instant};

use crate::audio::{LevelHandle, SignalHandle};
use crate::cleanup::CleanupMode;

/// Wie lange done/error sichtbar bleiben, bevor das Overlay verschwindet.
const DONE_SHOW: Duration = Duration::from_millis(700);
const ERROR_SHOW: Duration = Duration::from_millis(2500);
/// Antwortet das Mikrofon so lange nicht, wird der Start als Fehler gemeldet.
const STARTING_TIMEOUT: Duration = Duration::from_secs(5);
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
    /// Mikrofon startet (Balken statt Wellen); bleibt bis zum ersten Signal (`mic_started` + `tick`).
    Starting,
    /// Aufnahme läuft; trägt den für diese Utterance AUFGELÖSTEN Cleanup-Modus
    /// (Kontext-Awareness, Ticket-0026) — der Indicator ist der einzige
    /// Besitzer des Aufnahme-Status, das Tray leitet sein Icon daraus ab.
    Recording {
        mode: CleanupMode,
    },
    Transcribing,
    Done,
    Error(String),
}

pub struct Indicator {
    phase: Phase,
    phase_since: Instant,
    level: Option<LevelHandle>,
    smoothed_level: f32,
    /// Aufnahme läuft (`mic_started` lief, noch kein `transcribing`/`cancel`):
    /// ein Fehler blendet sich dann nicht aus (REQ-DICT-023).
    open: bool,
    signal: Option<SignalHandle>,
    mode: CleanupMode,
}

impl Default for Indicator {
    fn default() -> Self {
        Self {
            phase: Phase::Hidden,
            phase_since: Instant::now(),
            level: None,
            smoothed_level: 0.0,
            open: false,
            signal: None,
            mode: CleanupMode::Raw,
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

    /// PTT gedrückt, das Mikrofon läuft noch nicht.
    pub fn starting(&mut self, now: Instant) {
        self.phase = Phase::Starting;
        self.phase_since = now;
        self.level = None;
        self.signal = None;
        self.open = false;
    }

    /// Mikrofon läuft: Balken bleiben bis zum ersten Signal (REQ-DICT-016).
    /// Ein Fehler (Timeout war schneller) bleibt stehen; die Aufnahme läuft weiter.
    pub fn mic_started(&mut self, level: LevelHandle, signal: SignalHandle, mode: CleanupMode) {
        if !matches!(self.phase, Phase::Starting | Phase::Error(_)) {
            return;
        }
        self.open = true;
        self.level = Some(level);
        self.signal = Some(signal);
        self.mode = mode;
        if self.phase == Phase::Starting {
            self.switch_on_signal(self.phase_since);
        }
    }

    fn switch_on_signal(&mut self, now: Instant) -> bool {
        let arrived = self.signal.as_ref().is_some_and(SignalHandle::arrived);
        if arrived {
            self.phase = Phase::Recording { mode: self.mode };
            self.phase_since = now;
            self.smoothed_level = 0.0;
        }
        arrived
    }

    pub fn transcribing(&mut self, now: Instant) {
        self.phase = Phase::Transcribing;
        self.phase_since = now;
        self.level = None;
        self.signal = None;
        self.open = false;
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
        self.signal = None;
        self.open = false;
    }

    /// Zeitgesteuerte Übergänge (done/error blenden automatisch aus; ein
    /// hängender Mikrofon-Start wird zum Fehler).
    pub fn tick(&mut self, now: Instant) {
        if self.phase == Phase::Starting {
            if !self.switch_on_signal(now)
                && now.duration_since(self.phase_since) >= STARTING_TIMEOUT
            {
                self.fail(now, "Mikrofon antwortet nicht");
            }
            return;
        }
        if self.open && matches!(self.phase, Phase::Error(_)) {
            return;
        }
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

    /// Startet eine Aufnahme, deren Signal schon da ist (→ sofort Wellen).
    fn rec(ind: &mut Indicator, t: Instant) {
        ind.starting(t);
        let signal = SignalHandle::pending();
        signal.fire();
        ind.mic_started(LevelHandle::default(), signal, CleanupMode::Raw);
    }

    #[test]
    fn full_lifecycle_recording_to_done_hides_after_timeout() {
        let t0 = now();
        let mut ind = Indicator::default();
        assert!(!ind.visible());

        rec(&mut ind, t0);
        assert_eq!(
            *ind.phase(),
            Phase::Recording {
                mode: CleanupMode::Raw
            }
        );
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
        rec(&mut ind, t0);
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
        rec(&mut ind, t0);
        ind.ready(t0);
        assert!(
            matches!(ind.phase(), Phase::Recording { .. }),
            "ready darf Recording nicht stören"
        );
    }

    #[test]
    fn ptt_during_loading_switches_to_recording() {
        let t0 = now();
        let mut ind = Indicator::default();
        ind.loading(t0);
        rec(&mut ind, t0);
        assert_eq!(
            *ind.phase(),
            Phase::Recording {
                mode: CleanupMode::Raw
            }
        );
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
        rec(&mut ind, t0);
        ind.set_preview(false, t0);
        assert!(matches!(ind.phase(), Phase::Recording { .. }));
        // Preview-An während Recording wird ignoriert.
        ind.set_preview(true, t0);
        assert!(matches!(ind.phase(), Phase::Recording { .. }));

        ind.cancel();
        ind.set_preview(true, t0);
        assert_eq!(*ind.phase(), Phase::Preview);
        ind.set_preview(false, t0);
        assert!(!ind.visible());
    }

    /// Mikrofon-Start (ADR-0009-Nachtrag): Balken, bis die Aufnahme wirklich läuft.
    #[test]
    fn starting_shows_a_bar_until_the_recording_runs() {
        let mut ind = Indicator::default();
        let t0 = now();
        ind.starting(t0);
        assert_eq!(*ind.phase(), Phase::Starting);
        assert!(ind.visible());
        let signal = SignalHandle::pending();
        ind.mic_started(LevelHandle::default(), signal.clone(), CleanupMode::Raw);
        ind.tick(t0 + Duration::from_millis(100));
        assert_eq!(*ind.phase(), Phase::Starting, "Balken bis zum Signal");
        signal.fire();
        ind.tick(t0 + Duration::from_millis(200));
        assert_eq!(
            *ind.phase(),
            Phase::Recording {
                mode: CleanupMode::Raw
            }
        );
    }

    #[test]
    fn starting_can_be_cancelled_and_is_not_overridden_by_preview() {
        let mut ind = Indicator::default();
        ind.starting(now());
        ind.set_preview(true, now());
        assert_eq!(*ind.phase(), Phase::Starting);
        ind.cancel();
        assert!(!ind.visible());
    }

    /// Kein stiller Hänger: antwortet das Mikrofon nicht, erscheint ein Fehler.
    #[test]
    fn starting_times_out_into_a_visible_error() {
        let mut ind = Indicator::default();
        let t0 = now();
        ind.starting(t0);
        ind.tick(t0 + STARTING_TIMEOUT - Duration::from_millis(1));
        assert_eq!(*ind.phase(), Phase::Starting);
        ind.tick(t0 + STARTING_TIMEOUT);
        assert!(matches!(ind.phase(), Phase::Error(_)));
    }

    fn started_pending(t0: Instant) -> (Indicator, SignalHandle) {
        let mut ind = Indicator::default();
        ind.starting(t0);
        let signal = SignalHandle::pending();
        ind.mic_started(LevelHandle::default(), signal.clone(), CleanupMode::Raw);
        (ind, signal)
    }

    #[test]
    fn signal_in_the_first_block_switches_to_waves_at_once() {
        let mut ind = Indicator::default();
        ind.starting(now());
        let signal = SignalHandle::pending();
        signal.fire();
        ind.mic_started(LevelHandle::default(), signal, CleanupMode::Raw);
        assert!(matches!(ind.phase(), Phase::Recording { .. }));
    }

    #[test]
    fn signal_just_before_the_timeout_gives_waves_not_error() {
        let t0 = now();
        let (mut ind, signal) = started_pending(t0);
        ind.tick(t0 + STARTING_TIMEOUT - Duration::from_millis(1));
        assert_eq!(*ind.phase(), Phase::Starting);
        signal.fire();
        ind.tick(t0 + STARTING_TIMEOUT - Duration::from_millis(1));
        assert!(matches!(ind.phase(), Phase::Recording { .. }));
    }

    #[test]
    fn signal_exactly_at_the_timeout_wins_over_the_error() {
        let t0 = now();
        let (mut ind, signal) = started_pending(t0);
        signal.fire();
        ind.tick(t0 + STARTING_TIMEOUT);
        assert!(matches!(ind.phase(), Phase::Recording { .. }));
    }

    #[test]
    fn no_signal_at_the_timeout_shows_the_error_and_it_stays_while_recording() {
        let t0 = now();
        let (mut ind, _signal) = started_pending(t0);
        ind.tick(t0 + STARTING_TIMEOUT);
        assert_eq!(
            *ind.phase(),
            Phase::Error("Mikrofon antwortet nicht".into())
        );
        ind.tick(t0 + STARTING_TIMEOUT + Duration::from_secs(60));
        assert!(ind.visible(), "Fehler bleibt, solange die Aufnahme läuft");
    }

    #[test]
    fn late_signal_after_the_error_does_not_switch_to_waves() {
        let t0 = now();
        let (mut ind, signal) = started_pending(t0);
        ind.tick(t0 + STARTING_TIMEOUT);
        signal.fire();
        ind.tick(t0 + STARTING_TIMEOUT + Duration::from_secs(1));
        assert!(matches!(ind.phase(), Phase::Error(_)));
    }

    #[test]
    fn mic_started_after_the_timeout_keeps_the_error_visible() {
        let t0 = now();
        let mut ind = Indicator::default();
        ind.starting(t0);
        ind.tick(t0 + STARTING_TIMEOUT);
        let signal = SignalHandle::pending();
        signal.fire();
        ind.mic_started(LevelHandle::default(), signal, CleanupMode::Raw);
        assert!(matches!(ind.phase(), Phase::Error(_)));
        ind.tick(t0 + STARTING_TIMEOUT + Duration::from_secs(60));
        assert!(ind.visible());
    }

    #[test]
    fn release_before_the_first_signal_goes_to_transcribing() {
        let t0 = now();
        let (mut ind, _signal) = started_pending(t0);
        ind.transcribing(t0 + Duration::from_millis(500));
        assert_eq!(*ind.phase(), Phase::Transcribing);
        ind.tick(t0 + Duration::from_secs(60));
        assert_eq!(*ind.phase(), Phase::Transcribing);
    }

    #[test]
    fn release_after_the_error_makes_the_error_fade_normally() {
        let t0 = now();
        let (mut ind, _signal) = started_pending(t0);
        ind.tick(t0 + STARTING_TIMEOUT);
        ind.transcribing(t0 + STARTING_TIMEOUT);
        ind.fail(t0 + STARTING_TIMEOUT, "x");
        ind.tick(t0 + STARTING_TIMEOUT + Duration::from_millis(2600));
        assert!(!ind.visible());
    }

    #[test]
    fn failed_mic_start_hides_after_error_show() {
        let t0 = now();
        let mut ind = Indicator::default();
        ind.starting(t0);
        ind.fail(t0, "Mikrofon nicht verfügbar");
        ind.tick(t0 + Duration::from_millis(2600));
        assert!(!ind.visible());
    }

    #[test]
    fn mic_started_after_cancel_is_ignored() {
        let mut ind = Indicator::default();
        ind.starting(now());
        ind.cancel();
        let signal = SignalHandle::pending();
        signal.fire();
        ind.mic_started(LevelHandle::default(), signal, CleanupMode::Raw);
        assert!(!ind.visible());
    }

    #[test]
    fn cancel_hides_immediately_from_any_phase() {
        let t0 = now();
        let mut ind = Indicator::default();
        rec(&mut ind, t0);
        ind.cancel();
        assert!(!ind.visible());
    }

    #[test]
    fn tick_does_nothing_while_recording_or_transcribing() {
        let t0 = now();
        let mut ind = Indicator::default();
        rec(&mut ind, t0);
        ind.tick(t0 + Duration::from_secs(60));
        assert_eq!(
            *ind.phase(),
            Phase::Recording {
                mode: CleanupMode::Raw
            }
        );
    }

    #[test]
    fn smoothed_level_rises_fast_and_decays_slow() {
        let mut ind = Indicator::default();
        let h = LevelHandle::default();
        ind.starting(now());
        let signal = SignalHandle::pending();
        signal.fire();
        ind.mic_started(h.clone(), signal, CleanupMode::Raw);

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
