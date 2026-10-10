use crate::{
    config::Config,
    log,
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

mod accounting;
mod cleanup;
mod replay;
mod text;
use accounting::Accounting;
use cleanup::{complete_shutdown, stop};

const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// How often to try again while NVIDIA's `ShadowPlay` engine is not reachable (for example
/// when this app starts at logon before the NVIDIA App has finished starting).
const NVIDIA_RETRY_INTERVAL: Duration = Duration::from_secs(10);
/// How often to confirm that the engine still uses the RAM location.
const REDIRECTION_CHECK_INTERVAL: Duration = Duration::from_secs(5);

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
    WaitingForNvidia,
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
            Self::WaitingForNvidia => "Waiting for the NVIDIA App to start…",
            Self::Waiting => "RAM ready · waiting for Instant Replay writes",
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
    /// Instant Replay could not be restarted for the user; the location change itself succeeded.
    pub(crate) notice: Option<String>,
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
    notice: Option<String>,
    shutdown: Shutdown,
    /// The configuration to use when a start is retried.
    config: Config,
    /// When to try starting again because the engine was unreachable.
    retry: Option<Instant>,
    redirection_checked: Option<Instant>,
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
            notice: self.notice.clone(),
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
        config: config.clone(),
        ..State::default()
    };

    let startup = state.accounting.load(store).and_then(|()| {
        config.validate()?;
        start(store, config, &mut state)
    });
    if let Err(error) = startup {
        record_start_failure(&mut state, &error);
    }
    publish(output, &state);

    loop {
        let _ = receiver.recv_timeout(WORKER_POLL_INTERVAL);
        let command = match mailbox.lock() {
            Ok(mut mailbox) => mailbox.pending.take(),
            Err(_) => Some(Command::Shutdown),
        };

        match handle_command(store, command, &mut state, output) {
            CommandOutcome::Poll => {}
            CommandOutcome::Continue => continue,
            CommandOutcome::Exit => break,
        }

        retry_start_when_due(store, &mut state);

        if let Some(session) = &mut state.session
            && !session.stopping
            && let Err(error) = poll(store, session, &mut state.accounting, &mut state.message)
        {
            log::error(format!("RAM storage stopped unexpectedly: {error:#}"));
            let report = stop(store, &mut state);
            let mut message = format!("{error:#}");
            if !report.is_ok() {
                log::error(format!("Cleanup after the failure had problems: {report}"));
                let _ = write!(message, "\nCleanup: {report}");
            }
            state.error = Some(message);
        }

        keep_redirected(&mut state);
        publish(output, &state);
    }
}

/// An unreachable engine is not an error to show: the NVIDIA App may simply not have
/// started yet, so the start is retried until it is there.
fn record_start_failure(state: &mut State, error: &anyhow::Error) {
    let unreachable = error.chain().any(|cause| {
        cause
            .downcast_ref::<nvidia::NvidiaError>()
            .is_some_and(nvidia::NvidiaError::is_engine_unavailable)
    });
    if unreachable {
        if state.message != DisplayStatus::WaitingForNvidia {
            log::info("The NVIDIA App is not running yet; trying again every 10 seconds");
        }
        state.message = DisplayStatus::WaitingForNvidia;
        state.error = None;
        state.retry = Some(Instant::now() + NVIDIA_RETRY_INTERVAL);
    } else {
        log::error(format!("Could not start RAM storage: {error:#}"));
        state.message = DisplayStatus::StartFailed;
        state.error = Some(format!("{error:#}"));
        state.retry = None;
    }
}

/// Records how the Instant Replay cycle around a location change went.
pub(super) fn log_replay(outcome: &Result<replay::Outcome, String>) {
    match outcome {
        Ok(replay::Outcome::Restarted) => {
            log::info("Instant Replay was switched off and on so it records to the new location");
        }
        Ok(replay::Outcome::Untouched) => {
            log::info(
                "Instant Replay is not capturing right now; it picks up the new location when it starts",
            );
        }
        Err(error) => log::warning(text::replay_notice(error)),
    }
}

fn retry_start_when_due(store: &Store, state: &mut State) {
    retry_start_with(store, state, start);
}

/// Tries `start` again once the retry time has come and no session exists.
fn retry_start_with(
    store: &Store,
    state: &mut State,
    start: impl FnOnce(&Store, &Config, &mut State) -> Result<()>,
) {
    if state.session.is_some() || state.retry.is_none_or(|retry| Instant::now() < retry) {
        return;
    }

    state.retry = None;
    let config = state.config.clone();
    let attempt = state
        .accounting
        .load(store)
        .and_then(|()| start(store, &config, state));
    match attempt {
        Ok(()) => state.error = None,
        Err(error) => record_start_failure(state, &error),
    }
}

/// The overlay re-pushes its own stored location whenever it restarts, which would leave
/// NVIDIA recording to disk while the RAM drive is mounted; put the redirection back.
fn keep_redirected(state: &mut State) {
    keep_redirected_with(
        state,
        &mut replay::overlay(),
        nvidia::redirected,
        nvidia::apply,
    );
}

/// Every `REDIRECTION_CHECK_INTERVAL`, asks `redirected` whether the engine still records to
/// RAM and runs `apply` (around an Instant Replay cycle on `controls`) when it does not.
fn keep_redirected_with(
    state: &mut State,
    controls: &mut impl replay::Controls,
    redirected: impl FnOnce(&Redirect) -> Result<bool, nvidia::NvidiaError>,
    apply: impl FnOnce(&Redirect) -> Result<(), nvidia::NvidiaError>,
) {
    let Some(session) = &state.session else {
        return;
    };
    if session.stopping
        || state
            .redirection_checked
            .is_some_and(|checked| checked.elapsed() < REDIRECTION_CHECK_INTERVAL)
    {
        return;
    }
    state.redirection_checked = Some(Instant::now());

    // An unreachable engine is left alone; a value that changed is restored.
    if !matches!(redirected(&session.redirect), Ok(false)) {
        return;
    }
    log::warning(
        "NVIDIA switched its temporary files back to the original location (its overlay restarted); redirecting them to RAM again",
    );
    let redirect = session.redirect.clone();
    let (applied, replayed) = replay::around(controls, || apply(&redirect));
    state.notice = Some(match (applied, replayed) {
        (Err(error), _) => {
            let notice = text::redirect_again_failed(&format!("{error:#}"));
            log::error(&notice);
            notice
        }
        (Ok(()), replayed) => {
            log::info(format!("NVIDIA records to {} again", redirect.target));
            log_replay(&replayed);
            match replayed {
                Err(error) => text::replay_notice(&error),
                Ok(_) => text::REDIRECTED_AGAIN.to_owned(),
            }
        }
    });
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
            log::info(
                "Quitting: restoring NVIDIA's temporary files location and unmounting RAM storage",
            );
            state.shutdown = Shutdown::Pending;
            state.message = DisplayStatus::Stopping;
            publish(output, state);

            let report = stop(store, state);
            complete_shutdown(state, &report);
            publish(output, state);

            if state.shutdown == Shutdown::Complete {
                log::info("Shutdown complete");
                return CommandOutcome::Exit;
            }
            log::error(format!("Could not finish shutdown: {report}"));
        }
        Some(Command::Exit) => {
            log::warning("Exiting without waiting for cleanup to succeed");
            let _ = stop(store, state);
            return CommandOutcome::Exit;
        }
        Some(Command::Stop) => {
            log::info("Stop requested");
            state.retry = None;
            let report = stop(store, state);
            state.error = if report.is_ok() {
                log::info(
                    "RAM storage stopped; NVIDIA temporary files are back at their original location",
                );
                None
            } else {
                log::error(format!("Stop finished with problems: {report}"));
                Some(report.to_string())
            };
        }
        Some(Command::Start(config)) => {
            log::info(format!(
                "Start requested with a {} MB ceiling",
                config.memory_limit_mb
            ));
            if let Err(error) = config.validate() {
                log::error(format!("Settings rejected: {error}"));
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
    state.config = config.clone();
    state.retry = None;
    let report = stop(store, state);
    if !report.is_ok() {
        log::error(format!("Could not stop the previous session: {report}"));
        state.error = Some(report.to_string());
    } else if let Err(error) = state.accounting.load(store) {
        log::error(format!("Could not load the lifetime counters: {error:#}"));
        state.error = Some(format!("{error:#}"));
    } else if let Err(error) = start(store, config, state) {
        record_start_failure(state, &error);
    } else {
        state.error = None;
    }
}

fn publish(output: &Mutex<Status>, state: &State) {
    if let Ok(mut output) = output.lock() {
        *output = state.snapshot();
    }
}

pub(crate) fn recover_with(
    store: &Store,
    restore: impl FnOnce(&Redirect) -> Result<()>,
) -> Result<()> {
    if let Some(redirect) = store.redirect().context(text::READ_REDIRECT_JOURNAL)? {
        log::warning(format!(
            "An earlier run left NVIDIA pointing at RAM; restoring its temporary files to {}",
            redirect.original_path
        ));
        if let Err(error) = restore(&redirect).context(text::RECOVER_TEMP_PATH) {
            log::error(format!("Recovery failed: {error:#}"));
            return Err(error);
        }
        store
            .clear_redirect()
            .context(text::CLEAR_REDIRECT_JOURNAL)?;
        log::info("Recovery complete");
    }

    Ok(())
}

fn start(store: &Store, config: &Config, state: &mut State) -> Result<()> {
    state.notice = None;
    let redirect =
        nvidia::plan(config.target()).context("discover NVIDIA temporary files location")?;
    log::info(format!(
        "NVIDIA keeps its temporary files at {}; redirecting them to {}",
        redirect.original_path, redirect.target
    ));
    state.location = Some(redirect.clone());
    store.save_config(config)?;

    let helper = Helper::start(config, &redirect).context("mount RAM filesystem")?;
    log::info(format!(
        "RAM storage mounted at {} with a {} MB ceiling",
        Config::mount(),
        config.memory_limit_mb
    ));
    store
        .save_redirect(&redirect)
        .context("save redirect recovery journal")?;
    // Instant Replay reopens its files at the new location only after an off/on cycle.
    let (applied, replayed) = replay::around(&mut replay::overlay(), || nvidia::apply(&redirect));
    applied.context("redirect NVIDIA temporary files")?;
    log::info("NVIDIA now records to RAM");
    log_replay(&replayed);
    state.notice = replayed.err().map(|error| text::replay_notice(&error));

    state.memory_limit_bytes = Some(config.limit_bytes());
    state.message = DisplayStatus::Waiting;
    state.redirection_checked = Some(Instant::now());
    let now = Instant::now();
    state.session = Some(Session {
        helper,
        redirect,
        meter: Meter::new(state.accounting.total),
        sample: None,
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
        session.sample = Some(sample);
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
