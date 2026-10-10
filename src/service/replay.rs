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
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Scripted Instant Replay: each toggle flips the state after its scripted number of polls.
    struct Fake {
        running: bool,
        latencies: VecDeque<usize>,
        pending: Option<usize>,
        toggles: usize,
        pauses: usize,
        state_error: bool,
        toggle_fails: bool,
    }

    impl Fake {
        fn new(running: bool) -> Self {
            Self {
                running,
                latencies: VecDeque::new(),
                pending: None,
                toggles: 0,
                pauses: 0,
                state_error: false,
                toggle_fails: false,
            }
        }
    }

    impl Controls for Fake {
        type Error = String;

        fn running(&mut self) -> Result<bool, String> {
            if self.state_error {
                return Err("overlay unavailable".to_owned());
            }
            match self.pending {
                Some(0) => {
                    self.running = !self.running;
                    self.pending = None;
                }
                Some(remaining) => self.pending = Some(remaining - 1),
                None => {}
            }

            Ok(self.running)
        }

        fn toggle(&mut self) -> Result<(), String> {
            self.toggles += 1;
            if self.toggle_fails {
                return Err("hotkey unassigned".to_owned());
            }
            self.pending = Some(self.latencies.pop_front().unwrap_or(0));
            Ok(())
        }

        fn pause(&mut self, _: Duration) {
            self.pauses += 1;
        }
    }

    #[test]
    fn restarts_a_capturing_instant_replay_around_the_change() {
        let mut fake = Fake::new(true);
        fake.latencies = VecDeque::from(vec![3, 2]);
        let mut changed_while_stopped = None;

        let (changed, replay) = around(&mut fake, || {
            changed_while_stopped = Some(true);
            Ok::<_, String>(())
        });

        assert!(changed.is_ok());
        assert_eq!(replay, Ok(Outcome::Restarted));
        assert_eq!(fake.toggles, 2);
        assert!(fake.running);
        assert_eq!(changed_while_stopped, Some(true));
        // Three polls to see the stop, the grace pause before the restart, two polls to see the start.
        assert_eq!(fake.pauses, 6);
    }

    #[test]
    fn leaves_an_idle_instant_replay_alone() {
        let mut fake = Fake::new(false);

        let (changed, replay) = around(&mut fake, || Ok::<_, String>(7));

        assert_eq!(changed, Ok(7));
        assert_eq!(replay, Ok(Outcome::Untouched));
        assert_eq!(fake.toggles, 0);
    }

    #[test]
    fn replay_restarts_even_when_the_change_fails() {
        let mut fake = Fake::new(true);

        let (changed, replay) = around(&mut fake, || Err::<(), _>("engine refused"));

        assert_eq!(changed, Err("engine refused"));
        assert_eq!(replay, Ok(Outcome::Restarted));
        assert!(fake.running);
    }

    #[test]
    fn unreadable_state_skips_toggling_but_still_applies_the_change() {
        let mut fake = Fake::new(true);
        fake.state_error = true;

        let (changed, replay) = around(&mut fake, || Ok::<_, String>(()));

        assert!(changed.is_ok());
        assert!(replay.unwrap_err().contains("overlay unavailable"));
        assert_eq!(fake.toggles, 0);
    }

    #[test]
    fn failed_hotkey_is_reported_without_a_second_press() {
        let mut fake = Fake::new(true);
        fake.toggle_fails = true;

        let (changed, replay) = around(&mut fake, || Ok::<_, String>(()));

        assert!(changed.is_ok());
        assert!(replay.unwrap_err().contains("hotkey unassigned"));
        assert_eq!(fake.toggles, 1);
    }

    #[test]
    fn stop_that_never_settles_is_reported_and_not_pressed_again() {
        let mut fake = Fake::new(true);
        fake.latencies = VecDeque::from(vec![SETTLE_POLLS + 10]);

        let (changed, replay) = around(&mut fake, || Ok::<_, String>(()));

        assert!(changed.is_ok());
        assert!(replay.unwrap_err().contains("did not stop"));
        assert_eq!(fake.toggles, 1);
        assert_eq!(fake.pauses, SETTLE_POLLS);
    }

    #[test]
    fn restart_that_never_settles_is_reported() {
        let mut fake = Fake::new(true);
        fake.latencies = VecDeque::from(vec![0, SETTLE_POLLS + 10]);

        let (changed, replay) = around(&mut fake, || Ok::<_, String>(()));

        assert!(changed.is_ok());
        assert!(replay.unwrap_err().contains("did not start again"));
        assert_eq!(fake.toggles, 2);
    }

    #[test]
    fn unit_controls_never_toggle() {
        let (changed, replay) = around(&mut (), || Ok::<_, String>(1));

        assert_eq!(changed, Ok(1));
        assert_eq!(replay, Ok(Outcome::Untouched));
    }
}
