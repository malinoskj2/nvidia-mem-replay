use crate::APP_NAME;
use eframe::egui::{Context, ViewportCommand};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use thiserror::Error;
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};

#[derive(Debug, Error)]
pub(crate) enum TrayError {
    #[error("system tray: {0}")]
    Icon(#[from] tray_icon::Error),
    #[error("tray menu: {0}")]
    Menu(#[from] tray_icon::menu::Error),
    #[error("tray icon image: {0}")]
    Image(#[from] tray_icon::BadIcon),
}

pub(crate) struct Tray {
    _icon: TrayIcon,
    quitting: Arc<AtomicBool>,
    stop_item: MenuItem,
    quit_item: MenuItem,
}

impl Tray {
    pub(crate) fn new(
        ctx: &Context,
        stop: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, TrayError> {
        let menu = Menu::new();
        let show = MenuItem::new(format!("Show {APP_NAME}"), true, None);
        let stop_item = MenuItem::new("Stop and restore temporary path", true, None);
        let quit = MenuItem::new(format!("Quit {APP_NAME}"), true, None);
        menu.append_items(&[&show, &stop_item, &quit])?;

        let icon = TrayIconBuilder::new()
            .with_icon(replay_icon()?)
            .with_tooltip(format!("{APP_NAME} · click to open"))
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()?;

        let tray = Self {
            _icon: icon,
            quitting: Arc::new(AtomicBool::new(false)),
            stop_item,
            quit_item: quit,
        };
        tray.register_menu_handler(ctx, &show, stop);
        register_icon_handler(ctx);

        Ok(tray)
    }

    fn register_menu_handler(
        &self,
        ctx: &Context,
        show: &MenuItem,
        stop: impl Fn() + Send + Sync + 'static,
    ) {
        let quitting = Arc::clone(&self.quitting);
        let context = ctx.clone();
        let show_id = show.id().clone();
        let stop_id = self.stop_item.id().clone();
        let quit_id = self.quit_item.id().clone();

        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == show_id {
                show_window(&context);
            } else if event.id == stop_id {
                stop();
                show_window(&context);
            } else if event.id == quit_id {
                quitting.store(true, Ordering::Relaxed);
                show_window(&context);
                context.send_viewport_cmd(ViewportCommand::Close);
            }
            context.request_repaint();
        }));
    }

    pub(crate) fn quitting(&self) -> bool {
        self.quitting.load(Ordering::Relaxed)
    }

    pub(crate) fn disable_recording_controls(&self) {
        self.stop_item.set_enabled(false);
        self.quit_item.set_enabled(false);
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
        TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
    }
}

fn register_icon_handler(ctx: &Context) {
    let context = ctx.clone();
    TrayIconEvent::set_event_handler(Some(move |event| {
        if matches!(
            event,
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            }
        ) {
            show_window(&context);
            context.request_repaint();
        }
    }));
}

fn show_window(ctx: &Context) {
    // Hidden Windows windows cannot repaint to process queued viewport commands.
    let _ = winsafe::EnumThreadWindows(winsafe::GetCurrentThreadId(), |window| {
        if window.GetWindowText().is_ok_and(|title| title == APP_NAME) {
            window.ShowWindow(winsafe::co::SW::RESTORE);
            let _ = window.SetForegroundWindow();
            return false;
        }

        true
    });
    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(ViewportCommand::Focus);
}

fn replay_icon() -> Result<Icon, tray_icon::BadIcon> {
    const SIZE: u32 = 32;
    Icon::from_rgba(super::icon::rgba(SIZE), SIZE, SIZE)
}
