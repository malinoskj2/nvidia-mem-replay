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

const DRIVE_SELECTOR_ID: &str = "drive";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    Running,
    ShuttingDown,
    ExitReady,
}

pub(crate) struct App {
    tray: Option<Tray>,
    tray_error: Option<String>,
    worker: Worker,
    config: Config,
    settings: bool,
    lifecycle: Lifecycle,
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
            lifecycle: Lifecycle::Running,
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
        ui.label(RichText::new(text::status(status.message.as_str())).color(color));

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
        ui.add_enabled_ui(self.lifecycle == Lifecycle::Running, |ui| {
            ui.horizontal(|ui| {
                let label = if status.mounted {
                    text::STOP_AND_RESTORE
                } else {
                    text::RETRY_START
                };
                if ui.button(label).clicked() {
                    if status.mounted {
                        self.worker.stop();
                    } else {
                        self.worker.start(self.config.clone());
                    }
                }
                if ui.button(text::SETTINGS).clicked() {
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
                    .add_enabled(
                        self.lifecycle == Lifecycle::Running,
                        egui::Button::new(text::HIDE_TO_TRAY),
                    )
                    .clicked()
                {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                }
                if ui
                    .add_enabled(
                        self.lifecycle == Lifecycle::Running,
                        egui::Button::new(text::QUIT),
                    )
                    .clicked()
                {
                    tray.quit(ctx);
                }
            } else if ui
                .add_enabled(
                    self.lifecycle == Lifecycle::Running,
                    egui::Button::new(text::QUIT),
                )
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
        if self.lifecycle != Lifecycle::ShuttingDown {
            return;
        }

        if shutdown != Shutdown::Failed {
            ui.label(text::SHUTDOWN_PENDING);
            return;
        }

        ui.label(text::SHUTDOWN_FAILED_HELP);
        ui.horizontal(|ui| {
            if ui.button(text::RETRY_SHUTDOWN).clicked() {
                self.worker.shutdown();
            }
            if ui.button(text::EXIT_ANYWAY).clicked() {
                self.worker.exit();
                self.lifecycle = Lifecycle::ExitReady;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }

    fn show_settings(&mut self, ctx: &egui::Context) {
        if !self.settings || self.lifecycle != Lifecycle::Running {
            return;
        }

        egui::Window::new(text::SETTINGS_TITLE)
            .open(&mut self.settings)
            .show(ctx, |ui| {
                show_storage_settings(ui, &mut self.config);

                ui.label(text::RESTART_NOTICE);
                if ui.button(text::APPLY_AND_RESTART).clicked() {
                    self.worker.start(self.config.clone());
                }
            });
    }

    fn handle_close(&mut self, ctx: &egui::Context, shutdown: Shutdown) {
        if shutdown == Shutdown::Complete {
            self.lifecycle = Lifecycle::ExitReady;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        if !ctx.input(|input| input.viewport().close_requested())
            || self.lifecycle == Lifecycle::ExitReady
        {
            return;
        }

        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        if self.lifecycle != Lifecycle::Running {
            return;
        }

        if self.tray.as_ref().is_some_and(|tray| !tray.quitting()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            return;
        }

        self.lifecycle = Lifecycle::ShuttingDown;
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
            text::LIFETIME_WRITES,
            &text::lifetime_written(status.lifetime_bytes),
        );
        ui.add_space(24.0);
        let allocated =
            text::buffer_allocated(status.sample.as_ref().map(|sample| sample.buffer_bytes));
        metric(ui, text::ALLOCATED_BUFFER, &allocated);
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
        ui.small(text::original_location(path));
    }
    if let Some(path) = &status.target {
        ui.small(text::ram_location(path));
    }
    ui.small(text::buffer_ceiling(limit));
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
        ui.label(text::DRIVE);
        egui::ComboBox::from_id_salt(DRIVE_SELECTOR_ID)
            .selected_text(text::drive(config.drive))
            .show_ui(ui, |ui| {
                for letter in MIN_DRIVE..=MAX_DRIVE {
                    ui.selectable_value(&mut config.drive, letter, text::drive(letter));
                }
            });
        ui.label(text::MEMORY_CEILING);
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
