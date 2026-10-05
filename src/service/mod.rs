use crate::{
    config::Config,
    storage::Store,
    sys::{
        helper::{Helper, ShutdownReport},
        nvidia::{self, Redirect},
    },
    telemetry::{Meter, Sample},
};
use anyhow::{Context, Result};
use std::fmt::Write as _;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Shutdown {
    #[default]
    Idle,
    Pending,
    Failed,
    Complete,
}

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
    pub(crate) warning: Option<String>,
    pub(crate) shutdown: Shutdown,
    lifetime_dirty: bool,
}

enum Command {
    Start(Config),
    Stop,
    Shutdown,
    Exit,
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
    closing: Arc<AtomicBool>,
}

impl Control {
    fn request(&self, command: Command) {
        if let Ok(mut pending) = self.pending.lock() {
            if matches!(*pending, Command::Exit) {
                return;
            }
            if matches!(command, Command::Shutdown | Command::Exit) {
                self.closing.store(true, Ordering::Relaxed);
            } else if self.closing.load(Ordering::Relaxed) {
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
            closing: Arc::new(AtomicBool::new(false)),
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

    pub(crate) fn shutdown(&self) {
        self.control.request(Command::Shutdown);
    }

    pub(crate) fn exit(&self) {
        self.control.request(Command::Exit);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Window teardown is a fallback; normal Quit waits for published cleanup results.
        self.control.request(Command::Exit);
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
    stopping: bool,
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
                status.shutdown = Shutdown::Pending;
                "Stopping RAM storage…".clone_into(&mut status.message);
                publish(output, &status);
                let result = stop(store, &mut session, &mut status);
                complete_shutdown(&mut status, result);
                publish(output, &status);
                if status.shutdown == Shutdown::Complete {
                    break;
                }
            }
            Command::Exit => {
                // Best effort for unexpected teardown or an explicit exit after failure.
                let _ = stop(store, &mut session, &mut status);
                break;
            }
            Command::Stop => {
                status.error = stop(store, &mut session, &mut status)
                    .err()
                    .map(|error| format!("{error:#}"));
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
            && !running.stopping
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

pub(crate) fn recover(store: &Store) -> Result<()> {
    recover_with(store, |redirect| {
        nvidia::restore(redirect).map_err(Into::into)
    })
}

pub(crate) fn recover_with(
    store: &Store,
    restore: impl FnOnce(&Redirect) -> Result<()>,
) -> Result<()> {
    if let Some(redirect) = store.redirect().context("read redirect recovery journal")? {
        restore(&redirect).context("recover NVIDIA's original temporary location; retry restoration or set Temporary files to a persistent drive in Alt+Z")?;
        store
            .clear_redirect()
            .context("clear redirect recovery journal")?;
    }
    Ok(())
}

fn initialize(store: &Store, status: &mut Status) -> Result<()> {
    recover(store)?;
    // A failed checkpoint is still counted in memory across a restart.
    if !status.lifetime_dirty {
        status.lifetime_bytes = store.lifetime()?;
    }
    Ok(())
}

fn start(store: &Store, config: &Config, status: &mut Status) -> Result<Session> {
    config.validate()?;
    let redirect =
        nvidia::plan(config.target()).context("discover NVIDIA temporary files location")?;
    status.original_path = Some(redirect.original_path.clone());
    status.target = Some(redirect.target.clone());
    store.save_config(config)?;
    // Readiness acknowledges the same recovery snapshot used by the GUI and journal.
    let helper = Helper::start(config, &redirect).context("mount RAM filesystem")?;
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
        stopping: false,
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
    checkpoint(store, &mut session.checkpoint, status, now);
    Ok(())
}

fn checkpoint(store: &Store, last_attempt: &mut Instant, status: &mut Status, now: Instant) {
    if now.duration_since(*last_attempt) < Duration::from_secs(10) {
        return;
    }
    // Failures are retried at the same bounded cadence without unmounting RAM.
    *last_attempt = now;
    match store.save_lifetime(status.lifetime_bytes) {
        Ok(()) => {
            status.lifetime_dirty = false;
            status.warning = None;
        }
        Err(error) => {
            status.lifetime_dirty = true;
            status.warning = Some(format!(
                "Lifetime counter could not be saved; RAM recording continues. Retrying in 10 seconds: {error}"
            ));
        }
    }
}

fn stop(store: &Store, session: &mut Option<Session>, status: &mut Status) -> Result<()> {
    stop_with(store, session, status, |redirect| {
        nvidia::restore(redirect).map_err(Into::into)
    })
}

fn stop_with(
    store: &Store,
    session: &mut Option<Session>,
    status: &mut Status,
    restore: impl FnOnce(&Redirect) -> Result<()>,
) -> Result<()> {
    let Some(mut running) = session.take() else {
        let restoration = recover_with(store, restore);
        let persistence = persist_lifetime(store, status);
        let result = combine_errors([restoration, persistence]);
        if result.is_ok() {
            "Stopped · temporary location restored".clone_into(&mut status.message);
        }
        return result;
    };
    // Restore before unmounting; the journal remains if restoration fails.
    let restoration = restore(&running.redirect).context("restore NVIDIA temporary path; retry restoration or set Temporary files to a persistent drive in Alt+Z");
    let stopped = running.helper.stop();
    let exited = stopped.exited;
    let result = finish_stop(store, &mut running.meter, status, restoration, stopped);
    if !exited {
        running.stopping = true;
        *session = Some(running);
    }
    result
}

fn finish_stop(
    store: &Store,
    meter: &mut Meter,
    status: &mut Status,
    restoration: Result<()>,
    stopped: ShutdownReport,
) -> Result<()> {
    status.mounted = !stopped.exited;
    status.active = false;
    if stopped.exited {
        status.sample = None;
    }
    (if stopped.exited {
        "Stopped · cleanup needs attention"
    } else {
        "RAM helper is still running · retry stopping"
    })
    .clone_into(&mut status.message);
    let accounting = stopped.sample.as_ref().map_or(Ok(()), |sample| {
        meter
            .observe(sample, Instant::now())
            .map(|bytes| {
                status.lifetime_bytes = bytes;
            })
            .context("account final RAM writes")
    });
    status.lifetime_dirty = true;
    let persistence = persist_lifetime(store, status);
    let journal = if restoration.is_ok() && stopped.exited {
        store
            .clear_redirect()
            .context("clear redirect recovery journal")
    } else {
        Ok(())
    };
    let result = combine_errors([
        restoration,
        stopped.result.context("stop RAM filesystem"),
        accounting,
        persistence,
        journal,
    ]);
    if result.is_ok() {
        "Stopped · temporary location restored".clone_into(&mut status.message);
    }
    result
}

fn persist_lifetime(store: &Store, status: &mut Status) -> Result<()> {
    if status.lifetime_dirty {
        store.save_lifetime(status.lifetime_bytes).context(
            "save lifetime write counter; check application state directory access and retry",
        )?;
        status.lifetime_dirty = false;
        status.warning = None;
    }
    Ok(())
}

fn combine_errors<const N: usize>(results: [Result<()>; N]) -> Result<()> {
    let mut message = String::new();
    for error in results.into_iter().filter_map(Result::err) {
        if !message.is_empty() {
            message.push('\n');
        }
        let _ = write!(message, "{error:#}");
    }
    anyhow::ensure!(message.is_empty(), "{message}");
    Ok(())
}

fn complete_shutdown(status: &mut Status, result: Result<()>) {
    match result {
        Ok(()) => {
            status.shutdown = Shutdown::Complete;
            status.error = None;
        }
        Err(error) => {
            status.shutdown = Shutdown::Failed;
            status.error = Some(format!("Could not finish shutdown: {error:#}"));
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/service.rs"]
mod tests;
