use crate::{
    config::Config,
    storage::Store,
    sys::{
        helper::Helper,
        nvidia::{self, Redirect},
    },
    telemetry::{Meter, Sample},
};
use anyhow::{Context, Result};
use std::fmt::Write as _;
use std::{
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Default)]
pub(crate) struct Status {
    pub(crate) message: String,
    pub(crate) mounted: bool,
    pub(crate) active: bool,
    pub(crate) lifetime_bytes: u64,
    pub(crate) sample: Option<Sample>,
    pub(crate) original_path: Option<String>,
    pub(crate) target: Option<String>,
    pub(crate) memory_limit_bytes: Option<u64>,
    pub(crate) error: Option<String>,
}

enum Command {
    Start(Config),
    Stop,
    Shutdown,
    Idle,
}

pub(crate) struct Worker {
    control: Control,
    status: Arc<Mutex<Status>>,
    join: Option<JoinHandle<()>>,
}

#[derive(Clone)]
struct Control {
    pending: Arc<Mutex<Command>>,
    wake: SyncSender<()>,
}

impl Control {
    fn request(&self, command: Command) {
        if let Ok(mut pending) = self.pending.lock() {
            if matches!(*pending, Command::Shutdown) {
                return;
            }
            // The most recent desired state replaces any earlier pending request.
            *pending = command;
            // A full channel already contains a wake-up; the command itself is retained.
            let _ = self.wake.try_send(());
        }
    }
}

impl Worker {
    pub(crate) fn spawn(store: Store, config: Config) -> Self {
        let (wake, receiver) = mpsc::sync_channel(1);
        let control = Control {
            pending: Arc::new(Mutex::new(Command::Idle)),
            wake,
        };
        let status = Arc::new(Mutex::new(Status {
            message: "Preparing RAM storage…".to_owned(),
            ..Status::default()
        }));
        let output = Arc::clone(&status);
        let pending = Arc::clone(&control.pending);
        let join = thread::spawn(move || run(&store, &config, &receiver, &pending, &output));
        Self {
            control,
            status,
            join: Some(join),
        }
    }

    pub(crate) fn status(&self) -> Status {
        self.status.lock().map_or_else(
            |_| Status {
                error: Some("worker state unavailable".to_owned()),
                ..Status::default()
            },
            |status| status.clone(),
        )
    }

    pub(crate) fn start(&self, config: Config) {
        self.control.request(Command::Start(config));
    }

    pub(crate) fn stop_handle(&self) -> impl Fn() + Send + Sync + 'static {
        let control = self.control.clone();
        move || {
            control.request(Command::Stop);
        }
    }

    pub(crate) fn stop(&self) {
        self.control.request(Command::Stop);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.control.request(Command::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

struct Session {
    helper: Helper,
    redirect: Redirect,
    meter: Meter,
    last_sample: Instant,
    checkpoint: Instant,
}

fn run(
    store: &Store,
    config: &Config,
    receiver: &Receiver<()>,
    pending: &Mutex<Command>,
    output: &Mutex<Status>,
) {
    let mut status = Status {
        message: "Could not start RAM storage".to_owned(),
        ..Status::default()
    };
    let mut session = None;
    match initialize(store, &mut status).and_then(|()| start(store, config, &mut status)) {
        Ok(started) => session = Some(started),
        Err(error) => status.error = Some(format!("{error:#}")),
    }
    publish(output, &status);
    loop {
        let disconnected = matches!(
            receiver.recv_timeout(Duration::from_millis(250)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
        let command = if disconnected {
            Command::Shutdown
        } else {
            pending.lock().map_or(Command::Shutdown, |mut pending| {
                std::mem::replace(&mut *pending, Command::Idle)
            })
        };
        match command {
            Command::Shutdown => {
                if let Err(error) = stop(store, &mut session, &mut status) {
                    eprintln!("shutdown: {error:#}");
                }
                break;
            }
            Command::Stop => {
                if let Err(error) = stop(store, &mut session, &mut status) {
                    status.error = Some(format!("{error:#}"));
                }
            }
            Command::Start(config) => {
                if let Err(error) = config.validate() {
                    status.error = Some(error.to_string());
                    publish(output, &status);
                    continue;
                }
                let result = stop(store, &mut session, &mut status)
                    .and_then(|()| initialize(store, &mut status))
                    .and_then(|()| start(store, &config, &mut status));
                match result {
                    Ok(started) => {
                        session = Some(started);
                        status.error = None;
                    }
                    Err(error) => status.error = Some(format!("{error:#}")),
                }
            }
            Command::Idle => {}
        }
        if let Some(running) = &mut session
            && let Err(error) = poll(store, running, &mut status)
        {
            let mut message = format!("{error:#}");
            if let Err(cleanup) = stop(store, &mut session, &mut status) {
                let _ = write!(message, "\nCleanup: {cleanup:#}");
            }
            status.error = Some(message);
        }
        publish(output, &status);
    }
}

fn publish(output: &Mutex<Status>, status: &Status) {
    if let Ok(mut output) = output.lock() {
        *output = status.clone();
    }
}

fn initialize(store: &Store, status: &mut Status) -> Result<()> {
    // Recover a previous GUI/helper crash before discovering the next original path.
    if let Some(redirect) = store.redirect()? {
        nvidia::restore(&redirect).context("recover NVIDIA's original temporary location")?;
        store.clear_redirect()?;
    }
    status.lifetime_bytes = store.lifetime()?;
    Ok(())
}

fn start(store: &Store, config: &Config, status: &mut Status) -> Result<Session> {
    config.validate()?;
    let redirect =
        nvidia::plan(config.target()).context("discover NVIDIA temporary files location")?;
    status.original_path = Some(redirect.original_path.clone());
    status.target = Some(redirect.target.clone());
    store.save_config(config)?;
    // The helper snapshots the original registry value before publishing readiness.
    let helper = Helper::start(config).context("mount RAM filesystem")?;
    store
        .save_redirect(&redirect)
        .context("save redirect recovery journal")?;
    if let Err(error) = nvidia::apply(&redirect) {
        // Keep the journal until recovery confirms the setting was restored.
        return Err(error).context("redirect NVIDIA temporary files");
    }
    status.memory_limit_bytes = Some(config.limit_bytes());
    status.mounted = true;
    "RAM ready · waiting for NVIDIA writes".clone_into(&mut status.message);
    let now = Instant::now();
    Ok(Session {
        helper,
        redirect,
        meter: Meter::new(status.lifetime_bytes),
        last_sample: now,
        checkpoint: now,
    })
}

fn poll(store: &Store, session: &mut Session, status: &mut Status) -> Result<()> {
    let now = Instant::now();
    if let Some(sample) = session.helper.sample()? {
        status.lifetime_bytes = session.meter.observe(&sample, now)?;
        session.last_sample = now;
        status.sample = Some(sample);
    }
    if now.duration_since(session.last_sample) > Duration::from_secs(3) {
        anyhow::bail!("RAM helper telemetry is stale");
    }
    status.active = session.meter.active(now);
    (if status.active {
        "Data is being written to RAM"
    } else {
        "RAM ready · no writes in the last 2 seconds"
    })
    .clone_into(&mut status.message);
    if now.duration_since(session.checkpoint) >= Duration::from_secs(10) {
        store
            .save_lifetime(status.lifetime_bytes)
            .context("checkpoint lifetime write counter")?;
        session.checkpoint = now;
    }
    Ok(())
}

fn stop(store: &Store, session: &mut Option<Session>, status: &mut Status) -> Result<()> {
    let Some(mut running) = session.take() else {
        return Ok(());
    };
    // Restore before unmounting; do not leave future recordings aimed at an absent drive.
    let restoration = nvidia::restore(&running.redirect).context("restore NVIDIA temporary path");
    let stopped = running.helper.stop().context("stop RAM filesystem");
    status.mounted = false;
    status.active = false;
    status.sample = None;
    "Stopped · restoring temporary location".clone_into(&mut status.message);
    if let Ok(Some(sample)) = &stopped {
        status.lifetime_bytes = running.meter.observe(sample, Instant::now())?;
    }
    store.save_lifetime(status.lifetime_bytes)?;
    restoration?;
    stopped?;
    store.clear_redirect()?;
    "Stopped · temporary location restored".clone_into(&mut status.message);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_control_request_survives_a_full_wake_queue() {
        let (wake, receiver) = mpsc::sync_channel(1);
        let control = Control {
            pending: Arc::new(Mutex::new(Command::Idle)),
            wake,
        };
        control.request(Command::Start(Config::default()));
        control.request(Command::Stop);
        assert!(receiver.try_recv().is_ok());
        assert!(matches!(*control.pending.lock().unwrap(), Command::Stop));
    }

    #[test]
    fn shutdown_cannot_be_superseded_by_a_tray_or_window_action() {
        let (wake, _) = mpsc::sync_channel(1);
        let control = Control {
            pending: Arc::new(Mutex::new(Command::Idle)),
            wake,
        };
        control.request(Command::Shutdown);
        control.request(Command::Start(Config::default()));
        assert!(matches!(
            *control.pending.lock().unwrap(),
            Command::Shutdown
        ));
    }
}
