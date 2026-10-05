use crate::{
    config::Config,
    service::{Shutdown, Status, Worker},
    sys::tray::Tray,
};
use eframe::egui::{self, Color32, RichText};
use std::time::Duration;

pub(crate) struct App {
    tray: Option<Tray>,
    tray_error: Option<String>,
    worker: Worker,
    config: Config,
    settings: bool,
    quitting: bool,
    exit_ready: bool,
}

impl App {
    pub(crate) fn new(cc: &eframe::CreationContext<'_>, worker: Worker, config: Config) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let (tray, tray_error) = match Tray::new(&cc.egui_ctx, worker.stop_handle()) {
            Ok(tray) => (Some(tray), None),
            Err(error) => (
                None,
                Some(format!("Tray unavailable; closing will quit: {error}")),
            ),
        };
        Self {
            tray,
            tray_error,
            worker,
            config,
            settings: false,
            quitting: false,
            exit_ready: false,
        }
    }

    fn show_status(&self, ui: &mut egui::Ui, status: &Status) {
        ui.add_space(10.0);
        ui.heading("Replay in RAM");
        ui.label("NVIDIA Instant Replay temporary storage");
        ui.add_space(16.0);
        let color = if status.active {
            Color32::from_rgb(115, 225, 155)
        } else {
            Color32::GRAY
        };
        ui.label(RichText::new(format!("●  {}", status.message)).color(color));
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            metric(
                ui,
                "Written · lifetime",
                &format!("{:.3} GB", status.lifetime_bytes as f64 / 1e9),
            );
            ui.add_space(24.0);
            let allocated = status.sample.as_ref().map_or_else(
                || "— MB".to_owned(),
                |sample| format!("{:.1} MB", sample.buffer_bytes as f64 / 1e6),
            );
            metric(ui, "Buffer allocated", &allocated);
        });
        let limit = status
            .memory_limit_bytes
            .unwrap_or_else(|| self.config.limit_bytes());
        if let Some(sample) = &status.sample {
            ui.add_space(10.0);
            let resident = sample.resident_bytes.map_or_else(
                || "unavailable".to_owned(),
                |bytes| format!("{:.1} MB", bytes as f64 / 1e6),
            );
            ui.small(format!(
                "Filesystem process RAM: {resident} · System available: {:.0} MB",
                sample.available_bytes as f64 / 1e6
            ));
            if sample.available_bytes < 1_000_000_000 {
                ui.colored_label(
                    Color32::YELLOW,
                    "System memory is low. Reduce replay length or bitrate.",
                );
            }
            if sample.buffer_bytes > limit * 9 / 10 {
                ui.colored_label(
                    Color32::YELLOW,
                    "Near the memory ceiling. Recording may stop if the buffer fills.",
                );
            }
        }
        ui.add_space(16.0);
        if let Some(path) = &status.original_path {
            ui.small(format!("Detected: {path}"));
        }
        if let Some(path) = &status.target {
            ui.small(format!("RAM temporary files: {path}"));
        }
        ui.small(format!("Buffer ceiling: {} MB", limit / 1_000_000));
        ui.add_space(8.0);
        ui.label("If writes do not start, toggle Instant Replay off/on in Alt+Z. Keep Gallery on a persistent drive.");
        if let Some(warning) = &status.warning {
            ui.add_space(8.0);
            ui.colored_label(Color32::YELLOW, warning);
        }
        if let Some(error) = &status.error {
            ui.add_space(8.0);
            ui.colored_label(Color32::from_rgb(255, 155, 135), error);
        }
    }

    fn show_controls(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, status: &Status) {
        ui.add_space(12.0);
        ui.add_enabled_ui(!self.quitting, |ui| {
            ui.horizontal(|ui| {
                let label = if status.mounted {
                    "Stop and restore"
                } else {
                    "Retry / start"
                };
                if ui.button(label).clicked() {
                    if status.mounted {
                        self.worker.stop();
                    } else {
                        self.worker.start(self.config.clone());
                    }
                }
                if ui.button("Settings").clicked() {
                    self.settings = !self.settings;
                }
            });
        });
        if let Some(error) = &self.tray_error {
            ui.colored_label(Color32::YELLOW, error);
        }
        ui.horizontal(|ui| {
            if let Some(tray) = &self.tray {
                if ui
                    .add_enabled(!self.quitting, egui::Button::new("Hide to tray"))
                    .clicked()
                {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                }
                if ui
                    .add_enabled(!self.quitting, egui::Button::new("Quit"))
                    .clicked()
                {
                    tray.quit(ctx);
                }
            } else if ui
                .add_enabled(!self.quitting, egui::Button::new("Quit"))
                .clicked()
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
        if self.quitting {
            if status.shutdown == Shutdown::Failed {
                ui.label("Resolve the error and retry. If restoration failed, the recovery journal is kept for the next launch; you can also set Temporary files to a persistent drive in Alt+Z.");
                ui.horizontal(|ui| {
                    if ui.button("Retry shutdown").clicked() {
                        self.worker.shutdown();
                    }
                    if ui.button("Exit anyway").clicked() {
                        self.worker.exit();
                        self.exit_ready = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            } else {
                ui.label("Restoring the temporary path and stopping RAM storage…");
            }
        }
        ui.small(if self.tray.is_some() {
            "Closing hides to tray. Quit restores the path and discards the buffer."
        } else {
            "Closing restores the path and discards the RAM buffer."
        });
    }

    fn show_settings(&mut self, ctx: &egui::Context) {
        if self.settings && !self.quitting {
            egui::Window::new("RAM storage settings")
                .open(&mut self.settings)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Drive");
                        egui::ComboBox::from_id_salt("drive")
                            .selected_text(format!("{}:", self.config.drive))
                            .show_ui(ui, |ui| {
                                for letter in 'D'..='Z' {
                                    ui.selectable_value(
                                        &mut self.config.drive,
                                        letter,
                                        format!("{letter}:"),
                                    );
                                }
                            });
                        ui.label("Ceiling (MB)");
                        ui.add(
                            egui::DragValue::new(&mut self.config.memory_limit_mb)
                                .range(256..=65536),
                        );
                    });
                    ui.label(
                        "Save any wanted replay first; restarting discards the current buffer.",
                    );
                    if ui.button("Apply and restart").clicked() {
                        self.worker.start(self.config.clone());
                    }
                });
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        let status = self.worker.status();
        if status.shutdown == Shutdown::Complete {
            self.exit_ready = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if ctx.input(|input| input.viewport().close_requested()) && !self.exit_ready {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if !self.quitting && self.tray.as_ref().is_some_and(|tray| !tray.quitting()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else if !self.quitting {
                self.quitting = true;
                self.settings = false;
                self.worker.shutdown();
                if let Some(tray) = &self.tray {
                    tray.disable_recording_controls();
                }
            }
        }
        ctx.request_repaint_after(Duration::from_millis(250));
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.show_status(ui, &status);
                self.show_controls(ui, ctx, &status);
            });
        });
        self.show_settings(ctx);
    }
}

fn metric(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.vertical(|ui| {
        ui.small(label);
        ui.label(RichText::new(value).size(27.0).strong());
    });
}

pub(crate) struct StartupError(pub(crate) String);

impl eframe::App for StartupError {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Replay in RAM could not start");
            ui.label(&self.0);
            ui.label("Close this window, resolve the error, then launch again.");
        });
    }
}
