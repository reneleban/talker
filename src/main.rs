//! talker — lokaler macOS-Diktier-Assistent.
//! Pipeline: PTT-Hotkey → audio-capture → stt → cleanup → injection.
//! eframe besitzt den Main-Loop (Settings-Fenster, ADR-0002); Tray + Event-Tap
//! hängen am selben Main-RunLoop.

use std::cell::RefCell;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use talker::indicator::Indicator;
use talker::{
    audio, cleanup, clipboard, config, hotkey, injection, models, overlay, permissions, pipeline,
    stt, tray, ui,
};

thread_local! {
    /// Laufende Aufnahme zwischen PTT-Druck und -Loslassen (nur Main-Thread,
    /// da die Hotkey-Callbacks auf dem Main-RunLoop laufen).
    static RECORDING: RefCell<Option<audio::Recording>> = const { RefCell::new(None) };
    /// Frontmost-App beim Utterance-START (Ticket-0026) — beim Druck erfasst,
    /// damit ein App-Wechsel während der Aufnahme den Modus nicht ändert.
    static FRONTMOST: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Lädt das Cleanup-Modell; das zweite Element meldet den Fehlschlag
/// (Fehler-Cache: bis zum Moduswechsel nicht erneut versuchen).
fn try_load_cleaner() -> (Option<cleanup::GemmaCleaner>, bool) {
    let t = Instant::now();
    match cleanup::GemmaCleaner::new(&cleanup::GemmaCleaner::default_model_path()) {
        Ok(c) => {
            eprintln!(
                "talker: Cleanup-Modell geladen in {:.1}s.",
                t.elapsed().as_secs_f32()
            );
            (Some(c), false)
        }
        Err(e) => {
            eprintln!("talker: Cleanup nicht verfügbar — {e}");
            (None, true)
        }
    }
}

/// STT + Cleanup + Injection laufen abseits des Main-Threads: der Dictation
/// Worker (pipeline.rs) besitzt die Modelle und deren Lebenszyklus; dieser
/// Thread drainiert nur den Channel und mappt `Outcome` auf Indicator/Injection.
fn spawn_stt_worker(
    config: Arc<RwLock<config::Config>>,
    indicator: Arc<Mutex<Indicator>>,
    egui_ctx: eframe::egui::Context,
    models_state: Arc<models::ModelsState>,
) -> mpsc::Sender<(Vec<f32>, Option<String>)> {
    // Ergebnis der Pipeline ans Overlay melden (+ Repaint anstoßen).
    let report = move |f: &dyn Fn(&mut Indicator)| {
        if let Ok(mut ind) = indicator.lock() {
            f(&mut ind);
        }
        egui_ctx.request_repaint();
    };
    let (tx, rx) = mpsc::channel::<(Vec<f32>, Option<String>)>();
    std::thread::spawn(move || {
        let snapshot = |cfg: &Arc<RwLock<config::Config>>| cfg.read().map(|c| c.clone()).ok();

        let Some(cfg) = snapshot(&config) else { return };
        let mut worker = pipeline::DictationWorker::new(
            &cfg,
            models_state,
            Box::new(|dir: &std::path::Path| stt::ParakeetTranscriber::new(dir)),
            Box::new(try_load_cleaner),
        );
        report(&|ind| ind.ready(Instant::now()));

        for (pcm, frontmost) in rx {
            let Some(cfg) = snapshot(&config) else {
                // Poisoned Config-Lock: nicht still verwerfen (CLAUDE.md).
                eprintln!("talker: Config-Lock poisoned — Utterance verworfen.");
                report(&|ind| ind.fail(Instant::now(), "Interner Fehler (Config)"));
                continue;
            };
            match worker.handle(&pcm, frontmost.as_deref(), &cfg) {
                pipeline::Outcome::Inject {
                    text,
                    cleanup_fell_back,
                } => match injection::inject(
                    &clipboard::NsPasteboard,
                    &injection::CgKeySender,
                    &text,
                ) {
                    Ok(()) if cleanup_fell_back => {
                        // Sichtbar statt still (Ticket-0009): Text ist drin, aber roh.
                        report(&|ind| {
                            ind.fail(Instant::now(), "Cleanup übersprungen — Rohtext eingefügt")
                        });
                    }
                    Ok(()) => report(&|ind| ind.finish_ok(Instant::now())),
                    Err(e) => {
                        eprintln!("talker: Injection fehlgeschlagen: {e}");
                        report(&|ind| ind.fail(Instant::now(), "Einfügen fehlgeschlagen"));
                    }
                },
                pipeline::Outcome::Rejected(hint) => {
                    report(&|ind| ind.fail(Instant::now(), hint));
                }
            }
        }
    });
    tx
}

/// Startet talker als neuen Prozess und beendet diesen (Ticket-0030):
/// der Event-Tap funktioniert nach einem Laufzeit-Accessibility-Grant erst
/// im frischen Prozess. Scheitert das Spawnen, läuft die App weiter.
fn relaunch() {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("talker: Relaunch nicht möglich (current_exe): {e}");
            return;
        }
    };
    match std::process::Command::new(&exe).spawn() {
        Ok(_) => {
            eprintln!("talker: Accessibility erteilt — Neustart für den Event-Tap.");
            std::process::exit(0);
        }
        Err(e) => eprintln!("talker: Relaunch fehlgeschlagen: {e}"),
    }
}

fn main() -> ExitCode {
    let config = Arc::new(RwLock::new(config::Config::load()));

    // Modell-Setup (Ticket-0028/0029): Zustand per Präsenz-Check (der volle
    // Checksum-Check läuft im Hintergrund nach), unfertige Downloads bei
    // vorhandenem Consent direkt wieder anstoßen.
    let models_root = models::default_models_root();
    let consent = config
        .read()
        .map(|c| c.model_download_consent)
        .unwrap_or(false);
    let models_state = Arc::new(models::ModelsState::from_disk_quick(&models_root, consent));
    models::spawn_integrity_check(Arc::clone(&models_state), models_root.clone());
    models::start_needed_downloads(&models_state, &models_root, consent);

    // First-Run-Onboarding: fehlt eine Permission oder die Spracherkennung
    // (Setup/Consent), startet das Fenster sichtbar.
    let accessibility = permissions::ensure_accessibility();
    let mic_status = permissions::microphone_status();
    // Mic-Prompt sofort beim Erst-Start (Ticket-0030) — nicht erst beim
    // ersten PTT, der den Event-Tap (Accessibility + Relaunch) voraussetzt.
    if mic_status == permissions::MicPermission::Undetermined {
        permissions::request_microphone();
    }
    let mic_denied = mic_status == permissions::MicPermission::Denied;
    let show_onboarding = !accessibility || mic_denied || !models_state.stt_ready();

    let native_options = eframe::NativeOptions {
        // glow statt wgpu: wgpu kann auf macOS keine Fenster-Transparenz
        // (Overlay bekäme einen schwarzen Kasten) — egui#2680.
        renderer: eframe::Renderer::Glow,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("talker — Einstellungen")
            .with_inner_size([440.0, 600.0])
            .with_min_inner_size([400.0, 480.0])
            .with_visible(show_onboarding),
        event_loop_builder: Some(Box::new(|builder| {
            use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
            // Menüleisten-App: kein Dock-Icon.
            builder.with_activation_policy(ActivationPolicy::Accessory);
        })),
        ..Default::default()
    };

    let result = eframe::run_native(
        "talker",
        native_options,
        Box::new(move |cc| {
            // Läuft auf dem Main-Thread, der Event-Loop existiert bereits:
            // Tray, Event-Tap und Worker hier verdrahten.
            let indicator = Arc::new(Mutex::new(Indicator::default()));
            if let Ok(mut ind) = indicator.lock() {
                ind.loading(Instant::now());
            }
            let egui_ctx = cc.egui_ctx.clone();
            let initial_mode = config.read().map(|c| c.cleanup_mode).unwrap_or_default();
            let tray = Rc::new(tray::Tray::new(initial_mode)?);

            // Natives Overlay + Tray-Sync + 60-fps-Animations-Timer auf dem
            // Main-RunLoop — Tray-Sync ist idempotent (Cell-Diff in Tray::sync),
            // häufiges Aufrufen kostet nur Lesezugriffe (Ticket-0037).
            {
                use objc2::MainThreadMarker;
                use objc2_foundation::NSTimer;
                let mtm = MainThreadMarker::new().expect("creation ctx läuft auf Main-Thread");
                let overlay = Rc::new(overlay::Overlay::new(mtm));
                let ind = Arc::clone(&indicator);
                let cfg = Arc::clone(&config);
                let tray_sync = Rc::clone(&tray);
                let models_sync = Arc::clone(&models_state);
                // Soundness: Timer wird auf dem Main-RunLoop geplant und feuert
                // ausschließlich dort — der nicht-Send-Block verlässt den Thread nie.
                let block = block2::RcBlock::new(move |_timer: std::ptr::NonNull<NSTimer>| {
                    overlay.tick(&ind, &cfg);
                    let mode = cfg.read().map(|c| c.cleanup_mode).unwrap_or_default();
                    if let Ok(indicator) = ind.lock() {
                        tray_sync.sync(mode, indicator.phase(), models_sync.stt_ready());
                    }
                });
                let timer = unsafe {
                    NSTimer::scheduledTimerWithTimeInterval_repeats_block(1.0 / 60.0, true, &block)
                };
                std::mem::forget(timer); // lebt so lange wie die App
            }
            tray::set_instance(Rc::clone(&tray));

            if !accessibility {
                tray.set_permission_warning();
                eprintln!(
                    "talker: Accessibility-Permission fehlt — Onboarding im Settings-Fenster."
                );
            }

            let stt_tx = spawn_stt_worker(
                Arc::clone(&config),
                Arc::clone(&indicator),
                egui_ctx.clone(),
                Arc::clone(&models_state),
            );

            // Tap-Installation als wiederholbare Factory: scheitert sie (z.B.
            // Accessibility erst nach App-Start erteilt), versucht ein Timer es
            // alle 3 s erneut — kein App-Neustart nötig, kein stiller Tod.
            let install_tap = {
                let config = Arc::clone(&config);
                let indicator = Arc::clone(&indicator);
                let egui_ctx = egui_ctx.clone();
                let stt_tx = stt_tx.clone();
                let models_state = Arc::clone(&models_state);
                move || {
                    let mic_config = Arc::clone(&config);
                    let ind_press = Arc::clone(&indicator);
                    let ind_release = Arc::clone(&indicator);
                    let ctx_press = egui_ctx.clone();
                    let ctx_release = egui_ctx.clone();
                    let stt_tx = stt_tx.clone();
                    let models_press = Arc::clone(&models_state);
                    hotkey::install(
                        Arc::clone(&config),
                        move || {
                            // PTT gesperrt bis Parakeet ready (Ticket-0029):
                            // kurzer Hinweis mit Fortschritt statt Aufnahme.
                            if let Some(hint) =
                                ui::setup_hint(&models_press.get(models::ModelId::Parakeet))
                            {
                                eprintln!("talker: PTT gesperrt — {hint}");
                                if let Ok(mut ind) = ind_press.lock() {
                                    ind.fail(Instant::now(), hint);
                                }
                                ctx_press.request_repaint();
                                return;
                            }
                            // Frontmost-App beim Utterance-START erfassen (Ticket-0026):
                            // stabil, auch wenn während der Aufnahme die App wechselt.
                            let frontmost = injection::frontmost_bundle_id();
                            let resolved = mic_config
                                .read()
                                .map(|c| pipeline::resolve_mode(&c, frontmost.as_deref()))
                                .unwrap_or_default();
                            FRONTMOST.with(|f| *f.borrow_mut() = frontmost);
                            let device = mic_config.read().ok().and_then(|c| c.mic_device.clone());
                            match audio::start(device.as_deref()) {
                                Ok(rec) => {
                                    // Der Indicator besitzt den Aufnahme-Status
                                    // (inkl. aufgelöstem Modus); das Tray leitet
                                    // sein Icon daraus ab (Ticket-0035).
                                    if let Ok(mut ind) = ind_press.lock() {
                                        ind.start_recording(Instant::now(), rec.level(), resolved);
                                    }
                                    ctx_press.request_repaint();
                                    RECORDING.with(|r| *r.borrow_mut() = Some(rec));
                                }
                                Err(e) => {
                                    eprintln!("talker: {e}");
                                    tray::with_instance(|t| t.set_permission_warning());
                                }
                            }
                        },
                        move || {
                            let frontmost = FRONTMOST.with(|f| f.borrow_mut().take());
                            let Some(rec) = RECORDING.with(|r| r.borrow_mut().take()) else {
                                return; // Aufnahme kam nie zustande (z.B. Permission fehlte)
                            };
                            let update = |f: &dyn Fn(&mut Indicator)| {
                                if let Ok(mut ind) = ind_release.lock() {
                                    f(&mut ind);
                                }
                                ctx_release.request_repaint();
                            };
                            match rec.stop() {
                                Ok(pcm) => {
                                    let ms = audio::duration_ms(&pcm);
                                    if ms < audio::MIN_UTTERANCE_MS {
                                        eprintln!(
                                            "talker: Utterance zu kurz ({ms} ms) — verworfen."
                                        );
                                        update(&|ind| ind.cancel());
                                        return;
                                    }
                                    eprintln!(
                                        "talker: Utterance aufgenommen: {ms} ms, {} Samples @16 kHz.",
                                        pcm.len()
                                    );
                                    if stt_tx.send((pcm, frontmost)).is_err() {
                                        eprintln!(
                                            "talker: STT-Worker nicht verfügbar — Utterance verworfen."
                                        );
                                        update(&|ind| {
                                            ind.fail(Instant::now(), "STT nicht verfügbar")
                                        });
                                    } else {
                                        update(&|ind| ind.transcribing(Instant::now()));
                                    }
                                }
                                Err(e) => {
                                    eprintln!("talker: {e}");
                                    update(&|ind| {
                                        ind.fail(Instant::now(), "Aufnahme fehlgeschlagen")
                                    });
                                }
                            }
                        },
                    )
                }
            };

            match install_tap() {
                Ok(tap) => {
                    // Tap muss so lange leben wie die App.
                    std::mem::forget(tap);
                    eprintln!("talker: bereit — PTT-Taste halten = diktieren.");
                }
                Err(e) => {
                    tray::with_instance(|t| t.set_permission_warning());
                    eprintln!("talker: {e} — versuche alle 3 s erneut.");
                    use objc2_foundation::NSTimer;
                    // Soundness wie beim Overlay-Timer: geplant + gefeuert nur
                    // auf dem Main-RunLoop, der Block verlässt den Thread nie.
                    let models_relaunch = Arc::clone(&models_state);
                    let block = block2::RcBlock::new(move |timer: std::ptr::NonNull<NSTimer>| {
                        if let Ok(tap) = install_tap() {
                            std::mem::forget(tap);
                            tray::with_instance(|t| t.clear_permission_warning());
                            eprintln!("talker: Event-Tap nachträglich installiert — bereit.");
                            unsafe { timer.as_ref().invalidate() };
                        } else if permissions::should_relaunch_for_tap(
                            accessibility,
                            permissions::accessibility_granted(),
                        ) && !models_relaunch.any_download_running()
                        {
                            // TCC cached die Accessibility-Entscheidung pro
                            // Prozess — nach Laufzeit-Grant hilft nur ein
                            // Neustart (Ticket-0030). Laufende Modell-Downloads
                            // erst fertig laufen lassen (kein 5-GB-Abbruch).
                            relaunch();
                        }
                    });
                    let timer = unsafe {
                        NSTimer::scheduledTimerWithTimeInterval_repeats_block(3.0, true, &block)
                    };
                    std::mem::forget(timer);
                }
            }

            Ok(Box::new(ui::SettingsApp::new(
                Arc::clone(&config),
                tray.settings_id.clone(),
                tray.quit_id.clone(),
                !show_onboarding,
                Arc::clone(&indicator),
                Arc::clone(&models_state),
                models_root.clone(),
            )))
        }),
    );

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("talker: UI-Start fehlgeschlagen: {e}");
            ExitCode::FAILURE
        }
    }
}
