use crate::{
    config::{Config, MAX_DRIVE, MAX_MEMORY_LIMIT_MB, MIN_DRIVE, MIN_MEMORY_LIMIT_MB},
    service::{Shutdown, Status, Worker},
    sys::tray::Tray,
    telemetry::Sample,
};
use eframe::egui::{self, RichText};
use std::time::Duration;

mod text;
mod theme;

const DRIVE_SELECTOR_ID: &str = "drive";
const METRICS_GRID_ID: &str = "metrics";
const LOCATIONS_GRID_ID: &str = "locations";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lifecycle {
    Running,
    ShuttingDown,
    ExitReady,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Status,
    Settings,
}

pub(crate) struct App {
    tray: Option<Tray>,
    tray_error: Option<String>,
    worker: Worker,
    config: Config,
    tab: Tab,
    lifecycle: Lifecycle,
}

impl App {
    pub(crate) fn new(cc: &eframe::CreationContext<'_>, worker: Worker, config: Config) -> Self {
        theme::install(&cc.egui_ctx);
        let (tray, tray_error) = match Tray::new(&cc.egui_ctx, worker.stop_handle()) {
            Ok(tray) => (Some(tray), None),
            Err(error) => (None, Some(text::tray_unavailable(&error))),
        };

        Self {
            tray,
            tray_error,
            worker,
            config,
            tab: Tab::Status,
            lifecycle: Lifecycle::Running,
        }
    }

    fn limit(&self, status: &Status) -> u64 {
        status
            .memory_limit_bytes
            .unwrap_or_else(|| self.config.limit_bytes())
    }

    fn show_status_tab(&self, ui: &mut egui::Ui, status: &Status) {
        let limit = self.limit(status);

        theme::group_box(ui, text::GROUP_STATUS, |ui| {
            let color = if status.active {
                theme::OK_TEXT
            } else {
                theme::GRAY_TEXT
            };
            ui.label(RichText::new(text::status(status.message.as_str())).color(color));
            ui.add_space(2.0);
            egui::Grid::new(METRICS_GRID_ID)
                .num_columns(2)
                .spacing([16.0, 4.0])
                .show(ui, |ui| {
                    ui.label(text::LIFETIME_WRITES);
                    ui.label(theme::value(&text::lifetime_written(status.lifetime_bytes)));
                    ui.end_row();
                    ui.label(text::ALLOCATED_BUFFER);
                    ui.label(theme::value(&text::buffer_allocated(
                        status.sample.as_ref().map(|sample| sample.buffer_bytes),
                    )));
                    ui.end_row();
                });
            if let Some(sample) = &status.sample {
                show_memory_warnings(ui, sample, limit);
            }
            show_notices(ui, status);
        });

        theme::group_box(ui, text::GROUP_LOCATIONS, |ui| {
            let (message, color) = if status.mounted {
                (text::PATH_SWAPPED, theme::OK_TEXT)
            } else {
                (text::PATH_ORIGINAL, theme::GRAY_TEXT)
            };
            ui.label(RichText::new(text::status(message)).color(color));
            ui.add_space(2.0);
            egui::Grid::new(LOCATIONS_GRID_ID)
                .num_columns(2)
                .spacing([16.0, 4.0])
                .show(ui, |ui| {
                    ui.label(text::ORIGINAL_LOCATION);
                    ui.label(theme::value(status.original_path.as_deref().unwrap_or("—")));
                    ui.end_row();
                    ui.label(text::RAM_LOCATION);
                    ui.label(theme::value(status.target.as_deref().unwrap_or("—")));
                    ui.end_row();
                    ui.label(text::BUFFER_CEILING);
                    ui.label(theme::value(&text::buffer_ceiling(limit)));
                    ui.end_row();
                });
        });
    }

    fn show_settings_tab(&mut self, ui: &mut egui::Ui, status: &Status) {
        let running = self.lifecycle == Lifecycle::Running;

        theme::group_box(ui, text::GROUP_STORAGE, |ui| {
            show_storage_settings(ui, &mut self.config);
            ui.add_space(2.0);
            ui.label(RichText::new(text::RESTART_NOTICE).color(theme::GRAY_TEXT));
            ui.add_enabled_ui(running, |ui| {
                if ui.button(text::APPLY_AND_RESTART).clicked() {
                    self.worker.start(self.config.clone());
                }
            });
        });

        theme::group_box(ui, text::GROUP_RECORDING, |ui| {
            let (label, help) = if status.mounted {
                (text::STOP_AND_RESTORE, text::STOP_HELP)
            } else {
                (text::RETRY_START, text::START_HELP)
            };
            ui.label(help);
            ui.add_enabled_ui(running, |ui| {
                if ui.button(label).clicked() {
                    if status.mounted {
                        self.worker.stop();
                    } else {
                        self.worker.start(self.config.clone());
                    }
                }
            });
        });
    }

    fn show_footer(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, status: &Status) {
        ui.add_space(4.0);
        self.show_shutdown_controls(ui, ctx, status.shutdown);
        if let Some(error) = &self.tray_error {
            ui.label(RichText::new(error).color(theme::WARNING_TEXT));
        }
        ui.label(
            RichText::new(if self.tray.is_some() {
                text::CLOSE_WITH_TRAY
            } else {
                text::CLOSE_WITHOUT_TRAY
            })
            .small()
            .color(theme::GRAY_TEXT),
        );
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

        ui.label(RichText::new(text::SHUTDOWN_FAILED_HELP).color(theme::ERROR_TEXT));
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
        self.tab = Tab::Status;
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
                let selected = theme::tab_strip(
                    ui,
                    &[
                        (Tab::Status, text::TAB_STATUS),
                        (Tab::Settings, text::TAB_SETTINGS),
                    ],
                    &mut self.tab,
                );
                theme::tab_page(ui, selected, |ui| match self.tab {
                    Tab::Status => self.show_status_tab(ui, &status),
                    Tab::Settings => self.show_settings_tab(ui, &status),
                });
                self.show_footer(ui, ctx, &status);
            });
        });
    }
}

fn show_memory_warnings(ui: &mut egui::Ui, sample: &Sample, limit: u64) {
    if sample.available_bytes < 1_000_000_000 {
        ui.label(RichText::new(text::LOW_MEMORY_WARNING).color(theme::WARNING_TEXT));
    }
    if sample.buffer_bytes > limit * 9 / 10 {
        ui.label(RichText::new(text::BUFFER_LIMIT_WARNING).color(theme::WARNING_TEXT));
    }
}

fn show_notices(ui: &mut egui::Ui, status: &Status) {
    if status.mounted
        && let Some(warning) = &status.warning
    {
        ui.label(RichText::new(warning).color(theme::WARNING_TEXT));
    }
    if let Some(notice) = &status.notice {
        ui.label(RichText::new(notice).color(theme::WARNING_TEXT));
    }
    if let Some(error) = &status.error {
        ui.label(RichText::new(error).color(theme::ERROR_TEXT));
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

pub(crate) struct StartupError(String);

impl StartupError {
    pub(crate) fn new(cc: &eframe::CreationContext<'_>, message: String) -> Self {
        theme::install(&cc.egui_ctx);
        Self(message)
    }
}

impl eframe::App for StartupError {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            theme::group_box(ui, text::STARTUP_FAILED_TITLE, |ui| {
                ui.label(RichText::new(&self.0).color(theme::ERROR_TEXT));
                ui.label(text::STARTUP_FAILED_HELP);
            });
        });
    }
}
