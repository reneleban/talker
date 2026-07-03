//! Settings-Fenster (egui/eframe, ADR-0002) + First-Run-Permission-Onboarding.
//!
//! Das Fenster ist normalerweise unsichtbar; „Einstellungen…" im Tray-Menü
//! zeigt es. Config-Änderungen werden sofort gespeichert (TOML) und wirken
//! über `Arc<RwLock<Config>>` ohne Neustart (Hotkey, Cleanup, Mikrofon).

use std::process::Command;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use eframe::egui::{self, Align, Color32, FontData, FontDefinitions, FontFamily, Layout, RichText};
use tray_icon::menu::MenuEvent;

use std::sync::Mutex;
use std::time::Instant;

use crate::config::{self, Config, OverlayPosition};
use crate::indicator::Indicator;
use crate::login_item::{self, LoginItemStatus};
use crate::models::{self, ModelId, ModelState, ModelsState};
use crate::permissions;
use crate::{audio, hotkey, injection};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Allgemein,
    Vokabular,
    Kontext,
    Anzeige,
}

pub struct SettingsApp {
    config: Arc<RwLock<Config>>,
    settings_id: tray_icon::menu::MenuId,
    quit_id: tray_icon::menu::MenuId,
    mic_names: Vec<String>,
    stt_dir_input: String,
    vocab_input: String,
    save_note: Option<String>,
    /// true, wenn beim Start keine Permission fehlt → Fenster im ersten Frame
    /// verstecken (eframes with_visible(false) greift auf macOS nicht zuverlässig).
    hide_on_start: bool,
    first_frame: bool,
    quitting: bool,
    tab: Tab,
    indicator: Arc<Mutex<Indicator>>,
    preview_on: bool,
    /// Laufende Apps für den Kontext-Regel-Picker; aktualisiert beim Öffnen
    /// des Fensters und beim Wechsel auf den Kontext-Tab.
    running_apps: Vec<injection::RunningApp>,
    /// Geteilter Modell-Zustand (Ticket-0028/0029): Setup-Gate + Live-Status.
    models_state: Arc<ModelsState>,
    models_root: std::path::PathBuf,
    /// Zuletzt ans Tray gemeldeter Setup-Zustand (nur Änderungen senden).
    tray_setup: bool,
}

impl SettingsApp {
    pub fn new(
        config: Arc<RwLock<Config>>,
        settings_id: tray_icon::menu::MenuId,
        quit_id: tray_icon::menu::MenuId,
        hide_on_start: bool,
        indicator: Arc<Mutex<Indicator>>,
        models_state: Arc<ModelsState>,
        models_root: std::path::PathBuf,
    ) -> Self {
        let (stt_dir_input, vocab_input) = config
            .read()
            .map(|c| {
                (
                    c.stt_model_dir.to_string_lossy().into_owned(),
                    c.vocabulary.join("\n"),
                )
            })
            .unwrap_or_default();
        Self {
            config,
            settings_id,
            quit_id,
            mic_names: audio::input_device_names(),
            stt_dir_input,
            vocab_input,
            save_note: None,
            hide_on_start,
            first_frame: true,
            quitting: false,
            tab: Tab::Allgemein,
            indicator,
            preview_on: false,
            running_apps: injection::running_apps(),
            models_state,
            models_root,
            tray_setup: false,
        }
    }

    fn set_preview(&mut self, on: bool) {
        self.preview_on = on;
        if let Ok(mut ind) = self.indicator.lock() {
            ind.set_preview(on, Instant::now());
        }
    }

    /// macOS-naher Look: Systemschrift (SF), System-Settings-Grau, runde Ecken.
    fn apply_apple_style(ctx: &egui::Context) {
        // SF Pro von der Platte — schlägt die egui-Default-Schrift deutlich.
        if let Ok(bytes) = std::fs::read("/System/Library/Fonts/SFNS.ttf") {
            let mut fonts = FontDefinitions::default();
            fonts
                .font_data
                .insert("sf".into(), Arc::new(FontData::from_owned(bytes)));
            if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
                family.insert(0, "sf".into());
            }
            ctx.set_fonts(fonts);
        }
        ctx.all_styles_mut(|style| {
            use egui::{FontId, TextStyle};
            style
                .text_styles
                .insert(TextStyle::Body, FontId::proportional(13.0));
            style
                .text_styles
                .insert(TextStyle::Button, FontId::proportional(13.0));
            style
                .text_styles
                .insert(TextStyle::Small, FontId::proportional(11.0));
            style
                .text_styles
                .insert(TextStyle::Heading, FontId::proportional(20.0));
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(10.0, 4.0);
            let v = &mut style.visuals;
            let r = egui::CornerRadius::same(6);
            v.widgets.inactive.corner_radius = r;
            v.widgets.hovered.corner_radius = r;
            v.widgets.active.corner_radius = r;
            v.widgets.open.corner_radius = r;
            v.panel_fill = if v.dark_mode {
                Color32::from_rgb(30, 30, 32) // macOS Fenster-Grau (dunkel)
            } else {
                Color32::from_rgb(242, 242, 247) // macOS Fenster-Grau (hell)
            };
        });
    }

    /// Karten-Optik wie in den System-Einstellungen (weiße/graue Gruppe, rund).
    fn card(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) {
        let fill = if ui.visuals().dark_mode {
            Color32::from_rgb(44, 44, 46)
        } else {
            Color32::WHITE
        };
        egui::Frame::new()
            .fill(fill)
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(14, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                contents(ui);
            });
    }

    /// Eine Settings-Zeile: Label links, ⓘ-Hilfe daneben, Control rechtsbündig.
    fn row(ui: &mut egui::Ui, label: &str, hint: &str, control: impl FnOnce(&mut egui::Ui)) {
        ui.horizontal(|ui| {
            ui.set_min_height(26.0);
            ui.label(label);
            Self::hint(ui, hint);
            ui.with_layout(Layout::right_to_left(Align::Center), control);
        });
    }

    /// Kleines Hilfe-Symbol mit Hover-Erklärung. Bewusst ASCII „(?)" —
    /// Symbol-Glyphen wie ⓘ fehlen in der SF/egui-Fallback-Kette (Tofu).
    fn hint(ui: &mut egui::Ui, text: &str) {
        if text.is_empty() {
            return;
        }
        ui.label(RichText::new("(?)").weak().small())
            .on_hover_text(text);
    }

    fn section_title(ui: &mut egui::Ui, title: &str) {
        ui.add_space(12.0);
        ui.label(RichText::new(title).small().weak());
        ui.add_space(2.0);
    }

    fn hairline(ui: &mut egui::Ui) {
        let mut v = ui.visuals().widgets.noninteractive.bg_stroke;
        v.color = v.color.linear_multiply(0.5);
        ui.scope(|ui| {
            ui.visuals_mut().widgets.noninteractive.bg_stroke = v;
            ui.separator();
        });
    }

    /// Config unter Lock ändern, sofort speichern, Ergebnis anzeigen.
    fn update_config(&mut self, change: impl FnOnce(&mut Config)) {
        let Ok(mut cfg) = self.config.write() else {
            self.save_note = Some("Config-Lock vergiftet".into());
            return;
        };
        change(&mut cfg);
        self.save_note = Some(match cfg.save() {
            Ok(()) => "Gespeichert.".into(),
            Err(e) => format!("Speichern fehlgeschlagen: {e}"),
        });
    }

    fn permissions_section(&self, ui: &mut egui::Ui) {
        Self::section_title(ui, "BERECHTIGUNGEN");
        // Live-Status pro Frame; Anzeige-Logik ist pure + getestet (AK1).
        let rows = permissions::permission_rows(
            permissions::accessibility_granted(),
            permissions::microphone_status(),
        );
        Self::card(ui, |ui| {
            let n = rows.len();
            for (i, row) in rows.into_iter().enumerate() {
                let dot = if row.granted {
                    RichText::new("●").color(Color32::from_rgb(52, 199, 89))
                } else {
                    RichText::new("●").color(Color32::from_rgb(255, 159, 10))
                };
                ui.horizontal(|ui| {
                    ui.set_min_height(26.0);
                    ui.label(dot);
                    ui.label(row.label);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(pane) = row.pane
                            && ui.button("Systemeinstellungen …").clicked()
                        {
                            open_privacy_pane(pane);
                        }
                    });
                });
                if let Some(hint) = row.hint {
                    ui.label(RichText::new(hint).small().weak());
                }
                if i + 1 < n {
                    Self::hairline(ui);
                }
            }
        });
    }

    fn settings_section(&mut self, ui: &mut egui::Ui) {
        let snapshot = match self.config.read() {
            Ok(c) => c.clone(),
            Err(_) => return,
        };

        Self::section_title(ui, "AUFNAHME");
        Self::card(ui, |ui| {
            Self::row(
                ui,
                "Push-to-talk-Taste",
                "Diese Taste halten = aufnehmen, loslassen = Text einfügen. Nur Modifier-Tasten, da sie kein Tippen stören.",
                |ui| {
                    let current_label = hotkey::SELECTABLE_KEYS
                        .iter()
                        .find(|(code, _)| *code == snapshot.hotkey_keycode)
                        .map_or("unbekannt", |(_, label)| *label);
                    egui::ComboBox::from_id_salt("hotkey")
                        .selected_text(current_label)
                        .show_ui(ui, |ui| {
                            for (code, label) in hotkey::SELECTABLE_KEYS {
                                if ui
                                    .selectable_label(snapshot.hotkey_keycode == code, label)
                                    .clicked()
                                {
                                    self.update_config(|c| c.hotkey_keycode = code);
                                }
                            }
                        });
                },
            );
            Self::hairline(ui);
            Self::row(
                ui,
                "Mikrofon",
                "Aufnahmegerät. System-Default folgt der macOS-Einstellung; ein festes Gerät bleibt auch bei Wechseln aktiv.",
                |ui| {
                    let mic_label = snapshot.mic_device.as_deref().unwrap_or("System-Default");
                    egui::ComboBox::from_id_salt("mic")
                        .selected_text(mic_label)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(snapshot.mic_device.is_none(), "System-Default")
                                .clicked()
                            {
                                self.update_config(|c| c.mic_device = None);
                            }
                            for name in self.mic_names.clone() {
                                let selected =
                                    snapshot.mic_device.as_deref() == Some(name.as_str());
                                if ui.selectable_label(selected, &name).clicked() {
                                    self.update_config(|c| c.mic_device = Some(name.clone()));
                                }
                            }
                        });
                },
            );
            Self::hairline(ui);
            Self::row(
                ui,
                "Cleanup-Modus (gemma4:e2b)",
                "Stil der Bereinigung: Roh = wortwörtlich ohne LLM · Geschäftlich = formal, ohne Füllsel · Natürlich = nur Korrekturen, dein Ton bleibt · LLM-optimiert = macht aus Diktat einen strukturierten Prompt für z.B. Claude Code.",
                |ui| {
                    // Nicht-Roh-Modi ausgegraut, bis gemma ready ist — die
                    // Live-Aktivierung kommt über den geteilten ModelsState.
                    let llm_ok = self.models_state.llm_modes_available();
                    egui::ComboBox::from_id_salt("cleanup_mode")
                        .selected_text(snapshot.cleanup_mode.label())
                        .show_ui(ui, |ui| {
                            for mode in crate::cleanup::CleanupMode::ALL {
                                let enabled = !mode.uses_llm() || llm_ok;
                                if ui
                                    .add_enabled(
                                        enabled,
                                        egui::Button::selectable(
                                            snapshot.cleanup_mode == mode,
                                            mode.label(),
                                        ),
                                    )
                                    .clicked()
                                {
                                    self.update_config(|c| c.cleanup_mode = mode);
                                }
                            }
                        });
                },
            );
            if !self.models_state.llm_modes_available() {
                ui.label(
                    RichText::new(
                        "Nicht-Roh-Modi werden aktiv, sobald das Cleanup-Modell \
                         geladen ist (Status unter MODELLE).",
                    )
                    .small()
                    .weak(),
                );
            }
        });

        Self::section_title(ui, "MODELLE");
        Self::card(ui, |ui| {
            // Status je Modell + Neu laden/Reparieren (Ticket-0029, AK 4/5);
            // gemma-Hintergrund-Fortschritt erscheint als Balken unter der Zeile.
            for (id, label) in [
                (ModelId::Parakeet, "Spracherkennung (Parakeet)"),
                (ModelId::Gemma, "Cleanup-LLM (gemma4:e2b)"),
            ] {
                let state = self.models_state.get(id);
                let (text, action) = model_status_line(&state);
                Self::row(ui, label, "", |ui| {
                    if let Some(button_label) = action
                        && ui.button(button_label).clicked()
                    {
                        if snapshot.model_download_consent {
                            models::start_download(&self.models_state, id, &self.models_root, true);
                        } else {
                            self.save_note =
                                Some("Zuerst den Modell-Lizenzen zustimmen (Erst-Start).".into());
                        }
                    }
                    ui.label(RichText::new(text).small());
                });
                if let Some(frac) = progress_fraction(&state) {
                    ui.add(egui::ProgressBar::new(frac).show_percentage());
                }
                Self::hairline(ui);
            }
            ui.label("STT-Modell-Verzeichnis");
            ui.text_edit_singleline(&mut self.stt_dir_input);
            // In horizontal einpacken — sonst expandiert right_to_left vertikal
            // und die Karte frisst die volle Resthöhe.
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Übernehmen").clicked() {
                        let dir = std::path::PathBuf::from(self.stt_dir_input.trim());
                        self.update_config(|c| c.stt_model_dir = dir);
                    }
                    ui.label(RichText::new("lädt das Modell neu").small().weak());
                });
            });
        });

        Self::section_title(ui, "START");
        Self::card(ui, |ui| {
            Self::row(
                ui,
                "Beim Login starten",
                "Startet talker automatisch nach der Anmeldung (macOS-Anmeldeobjekt).",
                |ui| match login_item::status() {
                    LoginItemStatus::Unavailable => {
                        ui.label(
                            RichText::new("nur aus installiertem talker.app")
                                .small()
                                .weak(),
                        );
                    }
                    status => {
                        let mut enabled = status != LoginItemStatus::Disabled;
                        if ui.checkbox(&mut enabled, "").changed() {
                            self.save_note = Some(match login_item::set_enabled(enabled) {
                                Ok(()) => "Gespeichert.".into(),
                                Err(e) => e.to_string(),
                            });
                        }
                        if status == LoginItemStatus::RequiresApproval {
                            ui.label(
                                RichText::new("in Systemeinstellungen → Anmeldeobjekte bestätigen")
                                    .small()
                                    .weak(),
                            );
                        }
                    }
                },
            );
        });

        if let Some(note) = &self.save_note {
            ui.add_space(6.0);
            ui.label(RichText::new(note.clone()).small().weak());
        }
    }

    /// Tab „Vokabular": eigene Begriffe gegen STT-Verhörer.
    fn vokabular_tab(&mut self, ui: &mut egui::Ui) {
        let phonetic = self
            .config
            .read()
            .map(|c| c.phonetic_matching)
            .unwrap_or(true);
        Self::section_title(ui, "KORREKTUR");
        Self::card(ui, |ui| {
            Self::row(
                ui,
                "Phonetische Korrektur",
                "Ersetzt Verhörer deterministisch nach Klangbild (Kölner Phonetik), \
                 bevor das LLM läuft — wirkt dadurch auch im Roh-Modus \
                 (z.B. »Clotzelei« → »Claude CLI«). Ausgeschaltet korrigiert nur \
                 noch das LLM anhand der Begriffsliste.",
                |ui| {
                    let mut on = phonetic;
                    if ui.checkbox(&mut on, "").changed() {
                        self.update_config(|c| c.phonetic_matching = on);
                    }
                },
            );
        });

        Self::section_title(ui, "EIGENE BEGRIFFE");
        Self::card(ui, |ui| {
            ui.label(
                "Ein Begriff pro Zeile — der Cleanup korrigiert Verhörer der \
                 Spracherkennung auf exakt diese Schreibweise (z.B. »Clot Klee« → »Claude CLI«).",
            );
            ui.add_space(4.0);
            ui.add(
                egui::TextEdit::multiline(&mut self.vocab_input)
                    .desired_rows(12)
                    .desired_width(f32::INFINITY)
                    .hint_text("Claude CLI\negui\nKubernetes\nPadding"),
            );
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Übernehmen").clicked() {
                        // Trennzeichen-Reste (Komma/Semikolon) tolerieren.
                        let vocab: Vec<String> = self
                            .vocab_input
                            .lines()
                            .map(|l| l.trim().trim_matches([',', ';']).trim())
                            .filter(|l| !l.is_empty())
                            .map(str::to_string)
                            .collect();
                        self.update_config(|c| c.vocabulary = vocab);
                    }
                    ui.label(RichText::new("wirkt aufs nächste Diktat").small().weak());
                });
            });
        });
    }

    /// Tab „Kontext": Cleanup-Modus automatisch je fokussierter App (0027).
    fn kontext_tab(&mut self, ui: &mut egui::Ui) {
        let snapshot = match self.config.read() {
            Ok(c) => c.clone(),
            Err(_) => return,
        };

        Self::section_title(ui, "KONTEXT-AWARENESS");
        Self::card(ui, |ui| {
            Self::row(
                ui,
                "Modus automatisch je App wählen",
                "Opt-in: Beim Diktat-Start wird die fokussierte App erkannt und \
                 der Cleanup-Modus aus den Regeln unten gewählt. Ohne passende \
                 Regel gilt der manuell gewählte Modus (Allgemein/Tray).",
                |ui| {
                    let mut on = snapshot.context_aware_enabled;
                    if ui.checkbox(&mut on, "").changed() {
                        self.update_config(|c| c.context_aware_enabled = on);
                    }
                },
            );
            ui.label(
                RichText::new(
                    "Apps ohne Regel nutzen weiter den manuellen Cleanup-Modus \
                     (Fallback). Der Tray-Schnellwechsel ändert genau diesen Fallback.",
                )
                .small()
                .weak(),
            );
        });

        Self::section_title(ui, "REGELN (APP → MODUS)");
        Self::card(ui, |ui| {
            if snapshot.context_rules.is_empty() {
                ui.label(
                    RichText::new("Noch keine Regeln — unten eine laufende App wählen.")
                        .small()
                        .weak(),
                );
            }
            let n = snapshot.context_rules.len();
            let mut remove: Option<usize> = None;
            for (i, (bundle_id, mode)) in snapshot.context_rules.iter().enumerate() {
                // Klarname aus den laufenden Apps; sonst die bundle-id selbst.
                let display = self
                    .running_apps
                    .iter()
                    .find(|a| a.bundle_id == *bundle_id)
                    .map_or_else(|| bundle_id.clone(), |a| a.name.clone());
                ui.horizontal(|ui| {
                    ui.set_min_height(26.0);
                    ui.label(display).on_hover_text(bundle_id);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Entfernen").clicked() {
                            remove = Some(i);
                        }
                        egui::ComboBox::from_id_salt(("context_rule_mode", i))
                            .selected_text(mode.label())
                            .show_ui(ui, |ui| {
                                for m in crate::cleanup::CleanupMode::ALL {
                                    if ui.selectable_label(*mode == m, m.label()).clicked() {
                                        let id = bundle_id.clone();
                                        self.update_config(|c| {
                                            upsert_rule(&mut c.context_rules, &id, m);
                                        });
                                    }
                                }
                            });
                    });
                });
                if i + 1 < n {
                    Self::hairline(ui);
                }
            }
            if let Some(i) = remove {
                self.update_config(|c| {
                    if i < c.context_rules.len() {
                        c.context_rules.remove(i);
                    }
                });
            }
        });

        Self::section_title(ui, "REGEL HINZUFÜGEN");
        Self::card(ui, |ui| {
            Self::row(
                ui,
                "Laufende App",
                "Erfasst die bundle-id der gewählten App automatisch — du musst \
                 keine bundle-id kennen. Neue Regeln starten mit dem aktuellen \
                 manuellen Modus und lassen sich oben umstellen.",
                |ui| {
                    egui::ComboBox::from_id_salt("context_add_app")
                        .selected_text("App wählen …")
                        .show_ui(ui, |ui| {
                            for app in self.running_apps.clone() {
                                let already = snapshot
                                    .context_rules
                                    .iter()
                                    .any(|(id, _)| *id == app.bundle_id);
                                let label = if already {
                                    format!("{} (hat Regel)", app.name)
                                } else {
                                    app.name.clone()
                                };
                                if ui.selectable_label(false, label).clicked() && !already {
                                    let mode = snapshot.cleanup_mode;
                                    self.update_config(|c| {
                                        upsert_rule(&mut c.context_rules, &app.bundle_id, mode);
                                    });
                                }
                            }
                        });
                },
            );
            ui.label(
                RichText::new("Änderungen wirken aufs nächste Diktat.")
                    .small()
                    .weak(),
            );
        });

        if let Some(note) = &self.save_note {
            ui.add_space(6.0);
            ui.label(RichText::new(note.clone()).small().weak());
        }
    }

    /// Tab „Anzeige": Overlay-Position/-Breite mit Live-Vorschau.
    fn anzeige_tab(&mut self, ui: &mut egui::Ui) {
        let snapshot = match self.config.read() {
            Ok(c) => c.clone(),
            Err(_) => return,
        };

        Self::section_title(ui, "AUFNAHME-INDIKATOR");
        Self::card(ui, |ui| {
            Self::row(
                ui,
                "Vorschau",
                "Zeigt den Aufnahme-Indikator dauerhaft mit synthetischer Welle — zum Einstellen von Position, Breite und Optik ohne zu diktieren.",
                |ui| {
                    let mut on = self.preview_on;
                    if ui.checkbox(&mut on, "").changed() {
                        self.set_preview(on);
                    }
                },
            );
            ui.label(
                RichText::new(
                    "Zeigt den Indikator dauerhaft an der eingestellten Stelle — \
                     Änderungen wirken sofort.",
                )
                .small()
                .weak(),
            );
            Self::hairline(ui);
            Self::row(
                ui,
                "Position",
                "Wo der Indikator beim Diktieren erscheint: unten oder oben am Bildschirm der aktiven App.",
                |ui| {
                    let label = match snapshot.overlay_position {
                        OverlayPosition::Bottom => "Unten",
                        OverlayPosition::Top => "Oben",
                    };
                    egui::ComboBox::from_id_salt("overlay_pos")
                        .selected_text(label)
                        .show_ui(ui, |ui| {
                            for (pos, label) in [
                                (OverlayPosition::Bottom, "Unten"),
                                (OverlayPosition::Top, "Oben"),
                            ] {
                                if ui
                                    .selectable_label(snapshot.overlay_position == pos, label)
                                    .clicked()
                                {
                                    self.update_config(|c| c.overlay_position = pos);
                                }
                            }
                        });
                },
            );
            Self::hairline(ui);
            Self::row(
                ui,
                "Breite",
                "Breite des Indikators in Prozent der Bildschirmbreite.",
                |ui| {
                    let mut pct = snapshot.overlay_width_pct;
                    let slider =
                        egui::Slider::new(&mut pct, config::overlay_limits::WIDTH_PCT).suffix(" %");
                    if ui.add(slider).changed() {
                        self.update_config(|c| c.overlay_width_pct = pct);
                    }
                },
            );
        });

        Self::section_title(ui, "WELLEN-OPTIK");
        Self::card(ui, |ui| {
            Self::row(
                ui,
                "Empfindlichkeit",
                "Wie stark die Welle auf deine Lautstärke reagiert. Höher = größerer \
                 Ausschlag bei leiser Stimme.",
                |ui| {
                    let mut v = snapshot.overlay_gain;
                    if ui
                        .add(egui::Slider::new(&mut v, config::overlay_limits::GAIN).step_by(0.5))
                        .changed()
                    {
                        self.update_config(|c| c.overlay_gain = v);
                    }
                },
            );
            Self::hairline(ui);
            Self::row(
                ui,
                "Tempo",
                "Geschwindigkeit der Wellen-Animation (1,0 = normal).",
                |ui| {
                    let mut v = snapshot.overlay_speed;
                    if ui
                        .add(egui::Slider::new(&mut v, config::overlay_limits::SPEED).step_by(0.05))
                        .changed()
                    {
                        self.update_config(|c| c.overlay_speed = v);
                    }
                },
            );
            Self::hairline(ui);
            Self::row(
                ui,
                "Leuchtspur",
                "Anzahl der Nachbilder, die jede Welle hinter sich herzieht.",
                |ui| {
                    let mut v = snapshot.overlay_trail_len;
                    if ui
                        .add(egui::Slider::new(&mut v, config::overlay_limits::TRAIL_LEN))
                        .changed()
                    {
                        self.update_config(|c| c.overlay_trail_len = v);
                    }
                },
            );
            Self::hairline(ui);
            Self::row(
                ui,
                "Ausglühen",
                "Wie schnell die Leuchtspur verblasst: niedriger = kürzeres, \
                 höher = längeres Nachglühen.",
                |ui| {
                    let mut v = snapshot.overlay_trail_decay;
                    if ui
                        .add(
                            egui::Slider::new(&mut v, config::overlay_limits::TRAIL_DECAY)
                                .step_by(0.01),
                        )
                        .changed()
                    {
                        self.update_config(|c| c.overlay_trail_decay = v);
                    }
                },
            );
            Self::hairline(ui);
            Self::row(
                ui,
                "Farben",
                "Farben der vier Wellen-Linien (Überlagerungen leuchten additiv auf).",
                |ui| {
                    let mut colors = snapshot.overlay_colors.clone();
                    colors.resize(4, [255, 255, 255]);
                    let mut changed = false;
                    // Rechtsbündig → in umgekehrter Reihenfolge zeichnen.
                    for c in colors.iter_mut().rev() {
                        changed |= ui.color_edit_button_srgb(c).changed();
                    }
                    if changed {
                        self.update_config(|cfg| cfg.overlay_colors = colors);
                    }
                },
            );
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Optik zurücksetzen").clicked() {
                        self.update_config(|c| {
                            let d = Config::default();
                            c.overlay_gain = d.overlay_gain;
                            c.overlay_speed = d.overlay_speed;
                            c.overlay_trail_len = d.overlay_trail_len;
                            c.overlay_trail_decay = d.overlay_trail_decay;
                            c.overlay_colors = d.overlay_colors;
                        });
                    }
                });
            });
        });
    }

    /// Erst-Start-Setup (Ticket-0029): Lizenz-Consent → Parakeet-Fortschritt
    /// (blockiert die Nutzung) → bei Fehler sichtbar + Retry.
    fn setup_view(&mut self, ui: &mut egui::Ui, stage: SetupStage) {
        Self::section_title(ui, "EINRICHTUNG");
        Self::card(ui, |ui| match stage {
            SetupStage::Consent => {
                ui.label(
                    "talker lädt beim ersten Start zwei Modelle — alles läuft \
                     danach vollständig lokal auf diesem Mac:",
                );
                ui.add_space(4.0);
                ui.label("• Spracherkennung: Parakeet TDT 0.6b v3 (~0,5 GB)");
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Lizenz:").small().weak());
                    ui.hyperlink_to(
                        "CC-BY-4.0 (NVIDIA)",
                        "https://creativecommons.org/licenses/by/4.0/",
                    );
                });
                ui.label("• Cleanup-LLM: gemma4:e2b (~5 GB, lädt im Hintergrund)");
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Lizenz:").small().weak());
                    ui.hyperlink_to(
                        "Google Gemma Terms of Use",
                        "https://ai.google.dev/gemma/terms",
                    );
                });
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Mit „Akzeptieren\" stimmst du beiden Modell-Lizenzen zu \
                         (inkl. Gemma Prohibited-Use-Policy); danach starten die \
                         Downloads. Bis die Spracherkennung da ist, bleibt \
                         Push-to-talk deaktiviert.",
                    )
                    .small()
                    .weak(),
                );
                ui.add_space(4.0);
                if ui
                    .button("Lizenzen akzeptieren und Modelle laden")
                    .clicked()
                {
                    self.update_config(|c| c.model_download_consent = true);
                    // consent-pending → missing, dann Downloads anstoßen.
                    for id in [ModelId::Parakeet, ModelId::Gemma] {
                        if self.models_state.get(id) == ModelState::ConsentPending {
                            self.models_state.set(id, ModelState::Missing);
                        }
                    }
                    models::start_needed_downloads(&self.models_state, &self.models_root, true);
                }
            }
            SetupStage::Downloading => {
                let state = self.models_state.get(ModelId::Parakeet);
                ui.label(
                    "Die Spracherkennung wird eingerichtet — danach ist talker \
                     sofort nutzbar (Modus Roh).",
                );
                let (text, _) = model_status_line(&state);
                ui.label(RichText::new(text).small().weak());
                ui.add(
                    egui::ProgressBar::new(progress_fraction(&state).unwrap_or(0.0))
                        .show_percentage(),
                );
                ui.label(
                    RichText::new(
                        "gemma (Cleanup) lädt im Hintergrund weiter — Status \
                         unter MODELLE in den Einstellungen.",
                    )
                    .small()
                    .weak(),
                );
            }
            SetupStage::Failed(msg) => {
                ui.label(
                    RichText::new("Modell-Download fehlgeschlagen")
                        .color(Color32::from_rgb(255, 69, 58)),
                );
                ui.label(RichText::new(msg).small());
                if ui.button("Erneut versuchen").clicked() {
                    models::start_download(
                        &self.models_state,
                        ModelId::Parakeet,
                        &self.models_root,
                        true,
                    );
                }
            }
            SetupStage::Done => {}
        });
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (tab, label) in [
                (Tab::Allgemein, "Allgemein"),
                (Tab::Vokabular, "Vokabular"),
                (Tab::Kontext, "Kontext"),
                (Tab::Anzeige, "Anzeige"),
            ] {
                if ui.selectable_label(self.tab == tab, label).clicked() {
                    if tab == Tab::Kontext && self.tab != Tab::Kontext {
                        self.running_apps = injection::running_apps();
                    }
                    self.tab = tab;
                }
            }
        });
    }
}

impl eframe::App for SettingsApp {
    /// Transparent, damit das Overlay-Viewport keinen schwarzen Kasten bekommt;
    /// das Settings-Fenster malt seinen Hintergrund selbst (CentralPanel).
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.first_frame {
            self.first_frame = false;
            Self::apply_apple_style(ctx);
            if self.hide_on_start {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }

        // Tray-Menü-Events (Main-Thread) hier pollen — dafür regelmäßig aufwachen,
        // auch bei verstecktem Fenster (logic läuft auch dann).
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            // Modus-Schnellwechsel: Config ist die eine Quelle der Wahrheit,
            // das Tray wird unten per sync_mode nachgezogen.
            if let Some(mode) =
                crate::tray::with_instance_map(|t| t.mode_for_id(&event.id)).flatten()
            {
                self.update_config(|c| c.cleanup_mode = mode);
                continue;
            }
            if event.id == self.settings_id {
                self.mic_names = audio::input_device_names();
                self.running_apps = injection::running_apps();
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            } else if event.id == self.quit_id {
                self.quitting = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
        }

        // Fenster-Schließen = verstecken, App läuft weiter (Menüleisten-App) —
        // außer der Nutzer hat „Beenden" gewählt, dann das Close durchlassen.
        if !self.quitting && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            // Vorschau nicht verwaist weiterlaufen lassen.
            self.set_preview(false);
        }

        // Nach einem echten Diktat wieder in die Vorschau zurückfallen.
        if self.preview_on
            && let Ok(mut ind) = self.indicator.lock()
        {
            ind.set_preview(true, Instant::now());
        }

        // Tray spiegelt den Modus aus der Config (idempotent) — egal ob die
        // Änderung aus dem Settings-Dropdown oder dem Tray selbst kam.
        if let Ok(cfg) = self.config.read() {
            let mode = cfg.cleanup_mode;
            crate::tray::with_instance(|t| t.sync_mode(mode));
        }

        // Tray spiegelt den Aufnahme-Status aus der Indicator-Phase (Ticket-0035)
        // — nach sync_mode, damit ein Modus-Wechsel während der Aufnahme das
        // rote Icon nicht überschreibt. Idempotent, Besitzer ist der Indicator.
        if let Ok(ind) = self.indicator.lock() {
            let phase = ind.phase().clone();
            drop(ind);
            crate::tray::with_instance(|t| t.sync_recording(&phase));
        }

        // Tray-Setup-Icon (Ticket-0029): durchgestrichen, solange die
        // Spracherkennung fehlt — nur bei Zustands-Wechsel neu setzen.
        let setup_active = !self.models_state.stt_ready();
        if setup_active != self.tray_setup {
            self.tray_setup = setup_active;
            crate::tray::with_instance(|t| t.set_setup(setup_active));
        }

        ctx.request_repaint_after(Duration::from_millis(250));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(4.0);
                // Setup-Gate (Ticket-0029): bis Parakeet ready ist, ersetzt das
                // Erst-Start-Setup die Tabs — Consent, Fortschritt, Fehler.
                let consent = self
                    .config
                    .read()
                    .map(|c| c.model_download_consent)
                    .unwrap_or(false);
                let stage = setup_stage(consent, &self.models_state.get(ModelId::Parakeet));
                if stage != SetupStage::Done {
                    ui.heading("talker — Einrichtung");
                    self.permissions_section(ui);
                    self.setup_view(ui, stage);
                    if let Some(note) = &self.save_note {
                        ui.add_space(6.0);
                        ui.label(RichText::new(note.clone()).small().weak());
                    }
                    ui.add_space(8.0);
                    return;
                }
                ui.horizontal(|ui| {
                    ui.heading("talker");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        self.tab_bar(ui);
                    });
                });
                match self.tab {
                    Tab::Allgemein => {
                        self.permissions_section(ui);
                        self.settings_section(ui);
                    }
                    Tab::Vokabular => self.vokabular_tab(ui),
                    Tab::Kontext => self.kontext_tab(ui),
                    Tab::Anzeige => self.anzeige_tab(ui),
                }
                ui.add_space(8.0);
            });
        });
    }
}

/// Setup-Phase des Erst-Start-Fensters (Ticket-0029). Reine Logik, egui-frei:
/// Consent-Gate vor allem anderen, dann blockiert Parakeet bis `ready`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SetupStage {
    /// Lizenzen noch nicht akzeptiert — kein Download.
    Consent,
    /// Parakeet lädt/prüft; App-Nutzung bleibt gesperrt.
    Downloading,
    /// Parakeet-Download gescheitert — Fehler zeigen + Retry.
    Failed(String),
    /// Parakeet ready — Setup vorbei, normale Einstellungen.
    Done,
}

pub(crate) fn setup_stage(consent: bool, parakeet: &ModelState) -> SetupStage {
    if *parakeet == ModelState::Ready {
        return SetupStage::Done;
    }
    if !consent {
        return SetupStage::Consent;
    }
    match parakeet {
        ModelState::Corrupt => {
            SetupStage::Failed("Checksum-Prüfung fehlgeschlagen — die Datei war beschädigt.".into())
        }
        ModelState::Error(e) => SetupStage::Failed(e.clone()),
        _ => SetupStage::Downloading,
    }
}

/// Statuszeile eines Modells im „Modelle"-Bereich: (Text, Aktions-Button?).
/// Button-Beschriftung: „Neu laden" (fehlt) bzw. „Reparieren" (kaputt/Fehler).
pub(crate) fn model_status_line(state: &ModelState) -> (String, Option<&'static str>) {
    match state {
        ModelState::Ready => ("installiert ✓".into(), None),
        ModelState::Missing => ("fehlt".into(), Some("Neu laden")),
        ModelState::ConsentPending => ("wartet auf Lizenz-Zustimmung".into(), None),
        ModelState::Downloading { pct } => (format!("lädt … {pct} %"), None),
        ModelState::Verifying => ("wird geprüft …".into(), None),
        ModelState::Corrupt => ("beschädigt (Checksum)".into(), Some("Reparieren")),
        ModelState::Error(e) => (format!("Fehler: {e}"), Some("Reparieren")),
    }
}

/// Fortschritts-Anteil (0–1) fürs Balken-UI eines Modells; None = kein Balken.
pub(crate) fn progress_fraction(state: &ModelState) -> Option<f32> {
    match state {
        ModelState::Downloading { pct } => Some(f32::from(*pct) / 100.0),
        ModelState::Verifying => Some(1.0),
        _ => None,
    }
}

/// Hinweistext bei PTT-Druck, solange Parakeet nicht ready (AK 3).
/// None = PTT frei.
pub fn setup_hint(parakeet: &ModelState) -> Option<String> {
    match parakeet {
        ModelState::Ready => None,
        ModelState::Downloading { pct } => Some(format!("talker richtet sich ein … {pct} %")),
        ModelState::Verifying => Some("talker richtet sich ein … Modell wird geprüft".into()),
        ModelState::ConsentPending | ModelState::Missing => {
            Some("Einrichtung nötig — Einstellungen öffnen".into())
        }
        ModelState::Corrupt | ModelState::Error(_) => {
            Some("Modell-Download fehlgeschlagen — Einstellungen öffnen".into())
        }
    }
}

/// Fügt eine Kontext-Regel hinzu oder aktualisiert den Modus einer
/// bestehenden — bundle-ids bleiben einmalig, damit „erste Regel gewinnt"
/// (resolve_mode) nie mehrdeutig wird. Reine Logik, egui-frei (testbar).
pub(crate) fn upsert_rule(
    rules: &mut Vec<(String, crate::cleanup::CleanupMode)>,
    bundle_id: &str,
    mode: crate::cleanup::CleanupMode,
) {
    match rules.iter_mut().find(|(id, _)| id == bundle_id) {
        Some((_, m)) => *m = mode,
        None => rules.push((bundle_id.to_string(), mode)),
    }
}

fn open_privacy_pane(pane: &str) {
    let url = format!("x-apple.systempreferences:com.apple.preference.security?{pane}");
    if let Err(e) = Command::new("open").arg(url).spawn() {
        eprintln!("talker: Systemeinstellungen nicht öffenbar: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cleanup::CleanupMode;

    #[test]
    fn upsert_rule_adds_new_rules_in_order() {
        let mut rules = Vec::new();

        upsert_rule(&mut rules, "com.apple.Terminal", CleanupMode::LlmOptimized);
        upsert_rule(&mut rules, "com.apple.mail", CleanupMode::Business);

        assert_eq!(
            rules,
            vec![
                ("com.apple.Terminal".to_string(), CleanupMode::LlmOptimized),
                ("com.apple.mail".to_string(), CleanupMode::Business),
            ]
        );
    }

    #[test]
    fn upsert_rule_updates_existing_without_duplicate() {
        let mut rules = vec![
            ("com.apple.Terminal".to_string(), CleanupMode::LlmOptimized),
            ("com.apple.mail".to_string(), CleanupMode::Business),
        ];

        upsert_rule(&mut rules, "com.apple.Terminal", CleanupMode::Raw);

        assert_eq!(rules.len(), 2, "kein Duplikat");
        assert_eq!(
            rules[0],
            ("com.apple.Terminal".to_string(), CleanupMode::Raw)
        );
        assert_eq!(rules[1].0, "com.apple.mail", "Reihenfolge bleibt");
    }

    /// Consent-Gate (Ticket-0029, DoD): vor Accept nie Downloading — egal
    /// welcher Modell-Zustand; nach Accept blockiert Parakeet bis ready.
    #[test]
    fn setup_stage_gates_consent_before_any_download() {
        for state in [
            ModelState::Missing,
            ModelState::ConsentPending,
            ModelState::Corrupt,
            ModelState::Error("x".into()),
        ] {
            assert_eq!(setup_stage(false, &state), SetupStage::Consent, "{state:?}");
        }
        // Modell schon da (z.B. manuell installiert) → kein Consent nötig.
        assert_eq!(setup_stage(false, &ModelState::Ready), SetupStage::Done);
        assert_eq!(setup_stage(true, &ModelState::Ready), SetupStage::Done);
    }

    #[test]
    fn setup_stage_after_consent_downloads_and_surfaces_failures() {
        for state in [
            ModelState::Missing,
            ModelState::Downloading { pct: 42 },
            ModelState::Verifying,
        ] {
            assert_eq!(
                setup_stage(true, &state),
                SetupStage::Downloading,
                "{state:?}"
            );
        }
        assert!(matches!(
            setup_stage(true, &ModelState::Corrupt),
            SetupStage::Failed(_)
        ));
        let SetupStage::Failed(msg) = setup_stage(true, &ModelState::Error("Netz weg".into()))
        else {
            panic!("Error muss Failed ergeben");
        };
        assert_eq!(msg, "Netz weg");
    }

    /// Status→Darstellung-Mapping (Ticket-0029, DoD) — egui-frei.
    #[test]
    fn model_status_line_maps_every_state() {
        let cases: [(ModelState, &str, Option<&str>); 7] = [
            (ModelState::Ready, "installiert ✓", None),
            (ModelState::Missing, "fehlt", Some("Neu laden")),
            (
                ModelState::ConsentPending,
                "wartet auf Lizenz-Zustimmung",
                None,
            ),
            (ModelState::Downloading { pct: 7 }, "lädt … 7 %", None),
            (ModelState::Verifying, "wird geprüft …", None),
            (
                ModelState::Corrupt,
                "beschädigt (Checksum)",
                Some("Reparieren"),
            ),
            (
                ModelState::Error("Netz weg".into()),
                "Fehler: Netz weg",
                Some("Reparieren"),
            ),
        ];
        for (state, expected_text, expected_action) in cases {
            let (text, action) = model_status_line(&state);
            assert_eq!(text, expected_text, "{state:?}");
            assert_eq!(action, expected_action, "{state:?}");
        }
    }

    #[test]
    fn progress_fraction_only_for_running_downloads() {
        assert_eq!(
            progress_fraction(&ModelState::Downloading { pct: 50 }),
            Some(0.5)
        );
        assert_eq!(
            progress_fraction(&ModelState::Downloading { pct: 0 }),
            Some(0.0)
        );
        assert_eq!(progress_fraction(&ModelState::Verifying), Some(1.0));
        assert_eq!(progress_fraction(&ModelState::Ready), None);
        assert_eq!(progress_fraction(&ModelState::Missing), None);
    }

    /// PTT-Hinweis (AK 3): „richtet sich ein … X %" während des Downloads,
    /// None sobald Parakeet ready.
    #[test]
    fn setup_hint_reports_progress_and_clears_when_ready() {
        assert_eq!(setup_hint(&ModelState::Ready), None);
        assert_eq!(
            setup_hint(&ModelState::Downloading { pct: 42 }).unwrap(),
            "talker richtet sich ein … 42 %"
        );
        assert!(
            setup_hint(&ModelState::Verifying)
                .unwrap()
                .contains("geprüft")
        );
        for state in [ModelState::ConsentPending, ModelState::Missing] {
            assert!(
                setup_hint(&state).unwrap().contains("Einstellungen"),
                "{state:?}"
            );
        }
        for state in [ModelState::Corrupt, ModelState::Error("x".into())] {
            assert!(
                setup_hint(&state).unwrap().contains("fehlgeschlagen"),
                "{state:?}"
            );
        }
    }

    /// Regel-Add/Remove + Toggle persistieren durch die Config (AK-Test 0027).
    #[test]
    fn rule_editing_roundtrips_through_config_toml() {
        let mut cfg = Config {
            context_aware_enabled: true,
            ..Config::default()
        };
        upsert_rule(
            &mut cfg.context_rules,
            "com.apple.Terminal",
            CleanupMode::LlmOptimized,
        );
        upsert_rule(
            &mut cfg.context_rules,
            "com.apple.mail",
            CleanupMode::Casual,
        );
        cfg.context_rules.remove(1); // Remove ist ein schlichtes Vec::remove.

        let text = toml::to_string_pretty(&cfg).unwrap();
        let reloaded: Config = {
            // Über den offiziellen Lade-Pfad (RawConfig-Migration) gehen.
            let dir = std::env::temp_dir().join(format!("talker-ui-test-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("config.toml");
            std::fs::write(&path, &text).unwrap();
            let loaded = Config::load_from(&path);
            let _ = std::fs::remove_dir_all(&dir);
            loaded
        };

        assert!(reloaded.context_aware_enabled);
        assert_eq!(
            reloaded.context_rules,
            vec![("com.apple.Terminal".to_string(), CleanupMode::LlmOptimized)]
        );
    }
}
