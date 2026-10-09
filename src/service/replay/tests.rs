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
