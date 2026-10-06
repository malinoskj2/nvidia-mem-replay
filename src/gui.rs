use crate::{
    APP_NAME,
    config::{Config, MAX_DRIVE, MAX_MEMORY_LIMIT_MB, MIN_DRIVE, MIN_MEMORY_LIMIT_MB},
    service::{Shutdown, Status, Worker},
    sys::tray::Tray,
    telemetry::Sample,
};
use eframe::egui::{self, Color32, RichText};
use std::time::Duration;

mod text;

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
            Err(error) => (None, Some(text::tray_unavailable(&error))),
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
        ui.heading(APP_NAME);
        ui.label(text::SUBTITLE);
        ui.add_space(16.0);

        let color = if status.active {
            Color32::from_rgb(115, 225, 155)
        } else {
            Color32::GRAY
        };
        ui.label(RichText::new(format!("●  {}", status.message.as_str())).color(color));

        show_metrics(ui, status);

        let limit = status
            .memory_limit_bytes
            .unwrap_or_else(|| self.config.limit_bytes());
        if let Some(sample) = &status.sample {
            show_memory_status(ui, sample, limit);
        }

        show_location(ui, status, limit);
        show_notices(ui, status);
    }

    fn show_controls(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, status: &Status) {
        ui.add_space(12.0);
        self.show_recording_controls(ui, status);
        self.show_window_controls(ui, ctx);
        self.show_shutdown_controls(ui, ctx, status.shutdown);

        ui.small(if self.tray.is_some() {
            text::CLOSE_WITH_TRAY
        } else {
            text::CLOSE_WITHOUT_TRAY
        });
    }

    fn show_recording_controls(&mut self, ui: &mut egui::Ui, status: &Status) {
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
    }

    fn show_window_controls(&self, ui: &mut egui::Ui, ctx: &egui::Context) {
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
    }

    fn show_shutdown_controls(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        shutdown: Shutdown,
    ) {
        if !self.quitting {
            return;
        }

        if shutdown != Shutdown::Failed {
            ui.label(text::SHUTDOWN_PENDING);
            return;
        }

        ui.label(text::SHUTDOWN_FAILED_HELP);
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
    }

    fn show_settings(&mut self, ctx: &egui::Context) {
        if !self.settings || self.quitting {
            return;
        }

        egui::Window::new("RAM storage settings")
            .open(&mut self.settings)
            .show(ctx, |ui| {
                show_storage_settings(ui, &mut self.config);

                ui.label(text::RESTART_NOTICE);
                if ui.button("Apply and restart").clicked() {
                    self.worker.start(self.config.clone());
                }
            });
    }

    fn handle_close(&mut self, ctx: &egui::Context, shutdown: Shutdown) {
        if shutdown == Shutdown::Complete {
            self.exit_ready = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        if !ctx.input(|input| input.viewport().close_requested()) || self.exit_ready {
            return;
        }

        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        if self.quitting {
            return;
        }

        if self.tray.as_ref().is_some_and(|tray| !tray.quitting()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            return;
        }

        self.quitting = true;
        self.settings = false;
        self.worker.shutdown();
        if let Some(tray) = &self.tray {
            tray.disable_recording_controls();
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        let status = self.worker.status();
        self.handle_close(ctx, status.shutdown);

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

fn show_metrics(ui: &mut egui::Ui, status: &Status) {
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
}

fn show_memory_status(ui: &mut egui::Ui, sample: &Sample, limit: u64) {
    ui.add_space(10.0);
    ui.small(text::memory_summary(
        sample.resident_bytes,
        sample.available_bytes,
    ));
    if sample.available_bytes < 1_000_000_000 {
        ui.colored_label(Color32::YELLOW, text::LOW_MEMORY_WARNING);
    }
    if sample.buffer_bytes > limit * 9 / 10 {
        ui.colored_label(Color32::YELLOW, text::BUFFER_LIMIT_WARNING);
    }
}

fn show_location(ui: &mut egui::Ui, status: &Status, limit: u64) {
    ui.add_space(16.0);
    if let Some(path) = &status.original_path {
        ui.small(format!("Detected: {path}"));
    }
    if let Some(path) = &status.target {
        ui.small(format!("RAM temporary files: {path}"));
    }
    ui.small(format!("Buffer ceiling: {} MB", limit / 1_000_000));
    ui.add_space(8.0);
    ui.label(text::RECORDING_HELP);
}

fn show_notices(ui: &mut egui::Ui, status: &Status) {
    if let Some(warning) = &status.warning {
        ui.add_space(8.0);
        ui.colored_label(Color32::YELLOW, warning);
    }
    if let Some(error) = &status.error {
        ui.add_space(8.0);
        ui.colored_label(Color32::from_rgb(255, 155, 135), error);
    }
}

fn show_storage_settings(ui: &mut egui::Ui, config: &mut Config) {
    ui.horizontal(|ui| {
        ui.label("Drive");
        egui::ComboBox::from_id_salt("drive")
            .selected_text(format!("{}:", config.drive))
            .show_ui(ui, |ui| {
                for letter in MIN_DRIVE..=MAX_DRIVE {
                    ui.selectable_value(&mut config.drive, letter, format!("{letter}:"));
                }
            });
        ui.label("Ceiling (MB)");
        ui.add(
            egui::DragValue::new(&mut config.memory_limit_mb)
                .range(MIN_MEMORY_LIMIT_MB..=MAX_MEMORY_LIMIT_MB),
        );
    });
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
            ui.heading(text::STARTUP_FAILED_TITLE);
            ui.label(&self.0);
            ui.label(text::STARTUP_FAILED_HELP);
        });
    }
}
