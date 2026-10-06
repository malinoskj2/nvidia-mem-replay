use crate::{
    config::Config,
    storage::Store,
    sys::{
        helper::Helper,
        nvidia::{self, Redirect},
    },
    telemetry::{Meter, Sample, TELEMETRY_TIMEOUT},
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

mod accounting;
mod cleanup;
use accounting::Accounting;
use cleanup::{complete_shutdown, stop};

const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Shutdown {
    #[default]
    Idle,
    Pending,
    Failed,
    Complete,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) enum DisplayStatus {
    #[default]
    Preparing,
    StartFailed,
    Waiting,
    Writing,
    Ready,
    Stopping,
    Stopped,
    CleanupFailed,
    HelperRunning,
}

impl DisplayStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Preparing => "Preparing RAM storage…",
            Self::StartFailed => "Could not start RAM storage",
            Self::Waiting => "RAM ready · waiting for NVIDIA writes",
            Self::Writing => "Data is being written to RAM",
            Self::Ready => "RAM ready · no writes in the last 2 seconds",
            Self::Stopping => "Stopping RAM storage…",
            Self::Stopped => "Stopped · temporary location restored",
            Self::CleanupFailed => "Stopped · cleanup needs attention",
            Self::HelperRunning => "RAM helper is still running · retry stopping",
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct Status {
    pub(crate) message: DisplayStatus,
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
}

enum Command {
    Start(Config),
    Stop,
    Shutdown,
    Exit,
}

#[derive(Default)]
struct Mailbox {
    pending: Option<Command>,
    closing: bool,
}

pub(crate) struct Worker {
    control: Control,
    status: Arc<Mutex<Status>>,
    join: Option<JoinHandle<()>>,
}

#[derive(Clone)]
struct Control {
    mailbox: Arc<Mutex<Mailbox>>,
    wake: SyncSender<()>,
}

impl Control {
    fn request(&self, command: Command) {
        if let Ok(mut mailbox) = self.mailbox.lock() {
            if matches!(mailbox.pending, Some(Command::Exit)) {
                return;
            }
            if matches!(command, Command::Shutdown | Command::Exit) {
                mailbox.closing = true;
            } else if mailbox.closing {
                return;
            }
            // The latest desired state replaces pending work; Exit is final.
            mailbox.pending = Some(command);
            // A full channel already contains a wake-up; the request is retained.
            let _ = self.wake.try_send(());
        }
    }
}

impl Worker {
    pub(crate) fn spawn(store: Store, config: Config) -> Self {
        let (wake, receiver) = mpsc::sync_channel(1);
        let control = Control {
            mailbox: Arc::new(Mutex::new(Mailbox::default())),
            wake,
        };
        let status = Arc::new(Mutex::new(Status::default()));
        let output = Arc::clone(&status);
        let mailbox = Arc::clone(&control.mailbox);
        let join = thread::spawn(move || run(&store, &config, &receiver, &mailbox, &output));
        Self {
            control,
            status,
            join: Some(join),
        }
    }

    pub(crate) fn status(&self) -> Status {
        match self.status.lock() {
            Ok(status) => status.clone(),
            Err(_) => Status {
                error: Some("worker state unavailable".to_owned()),
                ..Status::default()
            },
        }
    }

    pub(crate) fn start(&self, config: Config) {
        self.control.request(Command::Start(config));
    }

    pub(crate) fn stop_handle(&self) -> impl Fn() + Send + Sync + 'static {
        let control = self.control.clone();
        move || control.request(Command::Stop)
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
        // Window teardown is a fallback; normal Quit waits for cleanup results.
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
    sample: Option<Sample>,
    last_sample: Instant,
    checkpoint: Instant,
    stopping: bool,
}

#[derive(Default)]
struct State {
    session: Option<Session>,
    accounting: Accounting,
    message: DisplayStatus,
    location: Option<Redirect>,
    memory_limit_bytes: Option<u64>,
    error: Option<String>,
    shutdown: Shutdown,
}

impl State {
    fn snapshot(&self) -> Status {
        let active = self
            .session
            .as_ref()
            .is_some_and(|session| !session.stopping && session.meter.active(Instant::now()));
        Status {
            message: self.message,
            mounted: self.session.is_some(),
            active,
            lifetime_bytes: self.accounting.total,
            sample: self
                .session
                .as_ref()
                .and_then(|session| session.sample.clone()),
            original_path: self
                .location
                .as_ref()
                .map(|redirect| redirect.original_path.clone()),
            target: self
                .location
                .as_ref()
                .map(|redirect| redirect.target.clone()),
            memory_limit_bytes: self.memory_limit_bytes,
            error: self.error.clone(),
            warning: self.accounting.warning.clone(),
            shutdown: self.shutdown,
        }
    }
}

fn run(
    store: &Store,
    config: &Config,
    receiver: &Receiver<()>,
    mailbox: &Mutex<Mailbox>,
    output: &Mutex<Status>,
) {
    let mut state = State {
        message: DisplayStatus::StartFailed,
        ..State::default()
    };
    if let Err(error) = initialize(store, &mut state) {
        state.error = Some(format!("{error:#}"));
    } else if let Err(error) = start(store, config, &mut state) {
        state.error = Some(format!("{error:#}"));
    }
    publish(output, &state);
    loop {
        let disconnected = matches!(
            receiver.recv_timeout(WORKER_POLL_INTERVAL),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
        let command = if disconnected {
            Some(Command::Shutdown)
        } else {
            match mailbox.lock() {
                Ok(mut mailbox) => mailbox.pending.take(),
                Err(_) => Some(Command::Shutdown),
            }
        };
        match handle_command(store, command, &mut state, output) {
            CommandOutcome::Poll => {}
            CommandOutcome::Continue => continue,
            CommandOutcome::Exit => break,
        }
        if let Some(session) = &mut state.session
            && !session.stopping
            && let Err(error) = poll(store, session, &mut state.accounting, &mut state.message)
        {
            let report = stop(store, &mut state);
            let mut message = format!("{error:#}");
            if !report.is_ok() {
                let _ = write!(message, "\nCleanup: {report}");
            }
            state.error = Some(message);
        }
        publish(output, &state);
    }
}

enum CommandOutcome {
    Poll,
    Continue,
    Exit,
}

fn handle_command(
    store: &Store,
    command: Option<Command>,
    state: &mut State,
    output: &Mutex<Status>,
) -> CommandOutcome {
    match command {
        Some(Command::Shutdown) => {
            state.shutdown = Shutdown::Pending;
            state.message = DisplayStatus::Stopping;
            publish(output, state);
            let report = stop(store, state);
            complete_shutdown(state, &report);
            publish(output, state);
            if state.shutdown == Shutdown::Complete {
                return CommandOutcome::Exit;
            }
        }
        Some(Command::Exit) => {
            let _ = stop(store, state);
            return CommandOutcome::Exit;
        }
        Some(Command::Stop) => {
            let report = stop(store, state);
            state.error = if report.is_ok() {
                None
            } else {
                Some(report.to_string())
            };
        }
        Some(Command::Start(config)) => {
            if let Err(error) = config.validate() {
                state.error = Some(error.to_string());
                publish(output, state);
                return CommandOutcome::Continue;
            }
            restart(store, &config, state);
        }
        None => {}
    }
    CommandOutcome::Poll
}

fn restart(store: &Store, config: &Config, state: &mut State) {
    let report = stop(store, state);
    if !report.is_ok() {
        state.error = Some(report.to_string());
    } else if let Err(error) = initialize(store, state) {
        state.error = Some(format!("{error:#}"));
    } else if let Err(error) = start(store, config, state) {
        state.error = Some(format!("{error:#}"));
    } else {
        state.error = None;
    }
}

fn publish(output: &Mutex<Status>, state: &State) {
    if let Ok(mut output) = output.lock() {
        *output = state.snapshot();
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

fn initialize(store: &Store, state: &mut State) -> Result<()> {
    recover(store)?;
    state.accounting.load(store)
}

fn start(store: &Store, config: &Config, state: &mut State) -> Result<()> {
    config.validate()?;
    let redirect =
        nvidia::plan(config.target()).context("discover NVIDIA temporary files location")?;
    state.location = Some(redirect.clone());
    store.save_config(config)?;
    let helper = Helper::start(config, &redirect).context("mount RAM filesystem")?;
    store
        .save_redirect(&redirect)
        .context("save redirect recovery journal")?;
    nvidia::apply(&redirect).context("redirect NVIDIA temporary files")?;
    state.memory_limit_bytes = Some(config.limit_bytes());
    state.message = DisplayStatus::Waiting;
    let now = Instant::now();
    state.session = Some(Session {
        helper,
        redirect,
        meter: Meter::new(state.accounting.total),
        sample: None,
        last_sample: now,
        checkpoint: now,
        stopping: false,
    });
    Ok(())
}

fn poll(
    store: &Store,
    session: &mut Session,
    accounting: &mut Accounting,
    message: &mut DisplayStatus,
) -> Result<()> {
    let now = Instant::now();
    if let Some(sample) = session.helper.sample()? {
        accounting.observe(&mut session.meter, &sample, now)?;
        session.last_sample = now;
        session.sample = Some(sample);
    }
    if now.duration_since(session.last_sample) > TELEMETRY_TIMEOUT {
        anyhow::bail!("RAM helper telemetry is stale");
    }
    *message = if session.meter.active(now) {
        DisplayStatus::Writing
    } else {
        DisplayStatus::Ready
    };
    accounting.checkpoint(store, &mut session.checkpoint, now);
    Ok(())
}

#[cfg(test)]
mod tests;
