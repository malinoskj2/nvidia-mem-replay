//! Restart Instant Replay around a location change so it reopens its files there.
//!
//! Instant Replay opens its temporary files when capture starts, so a location change takes
//! effect only after it is switched off and on. While it is capturing, the change is wrapped in
//! that cycle; while it is idle, the next capture already uses the new location.

use std::{fmt::Display, time::Duration};

const SETTLE_POLL: Duration = Duration::from_millis(100);
/// Five seconds of polling at `SETTLE_POLL`.
const SETTLE_POLLS: usize = 50;
/// The overlay ignores the hotkey while it is still tearing down the stopped capture session,
/// which outlasts the engine's "not capturing" report; a press 100 ms later was dropped.
const RESTART_GRACE: Duration = Duration::from_millis(1500);

/// The overlay's Instant Replay, observed and driven as the user would.
pub(crate) trait Controls {
    type Error: Display;

    /// Whether Instant Replay is capturing right now.
    fn running(&mut self) -> Result<bool, Self::Error>;

    /// Press the overlay's Instant Replay on/off hotkey.
    fn toggle(&mut self) -> Result<(), Self::Error>;

    fn pause(&mut self, duration: Duration);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Instant Replay was idle, so nothing had to be restarted.
    Untouched,
    /// Instant Replay was stopped before the change and is capturing again.
    Restarted,
}

/// Runs `change`, stopping Instant Replay first when it is capturing and starting it again
/// afterwards. The change always runs; replay problems are reported separately so that an
/// unassigned hotkey or an unreachable overlay never blocks the location change.
pub(crate) fn around<T, E>(
    controls: &mut impl Controls,
    change: impl FnOnce() -> Result<T, E>,
) -> (Result<T, E>, Result<Outcome, String>) {
    let stopped = match controls.running() {
        Ok(false) => Ok(false),
        Ok(true) => stop(controls).map(|()| true),
        Err(error) => Err(format!("read Instant Replay state: {error}")),
    };

    let changed = change();

    let replay = match stopped {
        Ok(false) => Ok(Outcome::Untouched),
        Ok(true) => start(controls).map(|()| Outcome::Restarted),
        // Whether the stop took effect is unknown, so a second press could switch it off.
        Err(error) => Err(error),
    };

    (changed, replay)
}

fn stop(controls: &mut impl Controls) -> Result<(), String> {
    controls
        .toggle()
        .map_err(|error| format!("stop Instant Replay: {error}"))?;
    settle(controls, false)
}

fn start(controls: &mut impl Controls) -> Result<(), String> {
    controls.pause(RESTART_GRACE);
    controls
        .toggle()
        .map_err(|error| format!("start Instant Replay: {error}"))?;
    settle(controls, true)
}

/// No Instant Replay to drive: the change simply runs.
impl Controls for () {
    type Error = std::convert::Infallible;

    fn running(&mut self) -> Result<bool, Self::Error> {
        Ok(false)
    }

    fn toggle(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn pause(&mut self, _: Duration) {}
}

/// The user's overlay, read through the `ShadowPlay` API and driven through its own hotkey.
#[cfg(windows)]
pub(crate) struct Overlay {
    chord: Option<crate::sys::hotkey::Chord>,
}

#[cfg(windows)]
impl Controls for Overlay {
    type Error = anyhow::Error;

    fn running(&mut self) -> Result<bool, Self::Error> {
        Ok(crate::sys::shadowplay::instant_replay_running()?)
    }

    fn toggle(&mut self) -> Result<(), Self::Error> {
        if self.chord.is_none() {
            self.chord = Some(crate::sys::hotkey::instant_replay_toggle()?);
        }
        let Some(chord) = &self.chord else {
            anyhow::bail!("Instant Replay hotkey unavailable");
        };

        crate::sys::hotkey::press(chord)?;
        Ok(())
    }

    fn pause(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

#[cfg(windows)]
pub(crate) fn overlay() -> Overlay {
    Overlay { chord: None }
}

#[cfg(not(windows))]
pub(crate) struct Overlay;

#[cfg(not(windows))]
impl Controls for Overlay {
    type Error = anyhow::Error;

    fn running(&mut self) -> Result<bool, Self::Error> {
        anyhow::bail!(crate::UNSUPPORTED_PLATFORM_MESSAGE)
    }

    fn toggle(&mut self) -> Result<(), Self::Error> {
        anyhow::bail!(crate::UNSUPPORTED_PLATFORM_MESSAGE)
    }

    fn pause(&mut self, _: Duration) {}
}

#[cfg(not(windows))]
pub(crate) const fn overlay() -> Overlay {
    Overlay
}

fn settle(controls: &mut impl Controls, running: bool) -> Result<(), String> {
    for _ in 0..SETTLE_POLLS {
        match controls.running() {
            Ok(state) if state == running => return Ok(()),
            Ok(_) => controls.pause(SETTLE_POLL),
            Err(error) => return Err(format!("read Instant Replay state: {error}")),
        }
    }

    Err(if running {
        "Instant Replay did not start again within 5 seconds".to_owned()
    } else {
        "Instant Replay did not stop within 5 seconds".to_owned()
    })
}

#[cfg(test)]
mod tests;
