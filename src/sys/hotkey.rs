//! The overlay's Instant Replay on/off hotkey, pressed through the standard Windows input queue.
//!
//! The overlay stores each hotkey in the `NVSPCAPS` registry key as `<name>Count` followed by
//! `<name>0`, `<name>1`, … virtual-key codes (`REG_BINARY` DWORDs), modifiers first. Pressing that
//! combination with `SendInput` is the same as the user pressing it: NVIDIA's hotkey helper
//! forwards it to the overlay, which stops or starts Instant Replay on its own capture session.

use std::io;
use thiserror::Error;
use winreg::{
    RegKey,
    enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WOW64_64KEY},
};
use winsafe::{HwKbMouse, KEYBDINPUT, SendInput, co};

const REGISTRY_KEY: &str = r"Software\NVIDIA Corporation\Global\ShadowPlay\NVSPCAPS";
const INSTANT_REPLAY_TOGGLE: &str = "IRToggleHKey";
const COUNT_SUFFIX: &str = "Count";
/// The overlay allows at most two modifiers and one key.
const MAX_KEYS: usize = 4;

#[derive(Debug, Error)]
pub(crate) enum HotkeyError {
    #[error(
        "the Instant Replay on/off hotkey is not set; assign one in Alt+Z > Settings > Keyboard shortcuts"
    )]
    Unassigned,
    #[error("read the Instant Replay hotkey: {0}")]
    Registry(#[from] io::Error),
    #[error("send the Instant Replay hotkey: {0}")]
    Input(co::ERROR),
}

/// Virtual-key codes pressed together, modifiers first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Chord(Vec<u16>);

impl Chord {
    /// Decodes the overlay's `Count` value and the key values that follow it.
    pub(crate) fn decode<'a>(
        count: &[u8],
        keys: impl IntoIterator<Item = &'a [u8]>,
    ) -> Result<Self, HotkeyError> {
        let count = dword(count).ok_or(HotkeyError::Unassigned)?;
        let wanted = usize::try_from(count).map_or(MAX_KEYS, |count| count.min(MAX_KEYS));
        (wanted != 0).ok_or(HotkeyError::Unassigned)?;

        let codes: Vec<u16> = keys
            .into_iter()
            .take(wanted)
            .map(|bytes| {
                dword(bytes)
                    .and_then(|code| u16::try_from(code).ok())
                    .filter(|code| *code != 0)
                    .ok_or(HotkeyError::Unassigned)
            })
            .collect::<Result<_, _>>()?;
        if codes.len() != wanted {
            return Err(HotkeyError::Unassigned);
        }

        Ok(Self(codes))
    }

    pub(crate) fn codes(&self) -> &[u16] {
        &self.0
    }
}

fn dword(bytes: &[u8]) -> Option<u32> {
    bytes
        .get(..4)
        .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// The hotkey the overlay currently assigns to Instant Replay on/off.
pub(crate) fn instant_replay_toggle() -> Result<Chord, HotkeyError> {
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(REGISTRY_KEY, KEY_READ | KEY_WOW64_64KEY)
        .map_err(unassigned_when_missing)?;
    let count = key
        .get_raw_value(format!("{INSTANT_REPLAY_TOGGLE}{COUNT_SUFFIX}"))
        .map_err(unassigned_when_missing)?;
    let mut keys = Vec::new();
    for index in 0..MAX_KEYS {
        match key.get_raw_value(format!("{INSTANT_REPLAY_TOGGLE}{index}")) {
            Ok(value) => keys.push(value.bytes.into_owned()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }

    Chord::decode(&count.bytes, keys.iter().map(Vec::as_slice))
}

fn unassigned_when_missing(error: io::Error) -> HotkeyError {
    if error.kind() == io::ErrorKind::NotFound {
        HotkeyError::Unassigned
    } else {
        HotkeyError::Registry(error)
    }
}

/// Presses and releases the chord as one input batch.
pub(crate) fn press(chord: &Chord) -> Result<(), HotkeyError> {
    let down = chord
        .codes()
        .iter()
        .map(|&code| event(code, co::KEYEVENTF::default()));
    let up = chord
        .codes()
        .iter()
        .rev()
        .map(|&code| event(code, co::KEYEVENTF::KEYUP));
    let events: Vec<HwKbMouse> = down.chain(up).collect();

    SendInput(&events).map_err(HotkeyError::Input)?;
    Ok(())
}

#[allow(unsafe_code)]
fn event(code: u16, flags: co::KEYEVENTF) -> HwKbMouse {
    // SAFETY: `VK` is a plain newtype over the virtual-key code; every value is representable.
    let key = unsafe { co::VK::from_raw(code) };

    HwKbMouse::Kb(KEYBDINPUT {
        wVk: key,
        dwFlags: flags,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dword_bytes(value: u32) -> Vec<u8> {
        value.to_le_bytes().to_vec()
    }

    #[test]
    fn decodes_count_then_modifiers_and_key_in_order() {
        let keys = [dword_bytes(0x12), dword_bytes(0x7b), dword_bytes(0x79)];

        let chord = Chord::decode(&dword_bytes(2), keys.iter().map(Vec::as_slice)).unwrap();

        // Stale entries beyond Count are ignored; Alt+F12 stays Alt+F12.
        assert_eq!(chord.codes(), &[0x12, 0x7b]);
    }

    #[test]
    fn rejects_unassigned_or_malformed_hotkeys() {
        let keys = [dword_bytes(0x12), dword_bytes(0x7b)];

        assert!(matches!(
            Chord::decode(&dword_bytes(0), keys.iter().map(Vec::as_slice)),
            Err(HotkeyError::Unassigned)
        ));
        assert!(matches!(
            Chord::decode(&[1, 0], keys.iter().map(Vec::as_slice)),
            Err(HotkeyError::Unassigned)
        ));
        assert!(matches!(
            Chord::decode(&dword_bytes(3), keys.iter().map(Vec::as_slice)),
            Err(HotkeyError::Unassigned)
        ));
        assert!(matches!(
            Chord::decode(&dword_bytes(1), [dword_bytes(0)].iter().map(Vec::as_slice)),
            Err(HotkeyError::Unassigned)
        ));
        assert!(matches!(
            Chord::decode(
                &dword_bytes(1),
                [dword_bytes(0x1_0000)].iter().map(Vec::as_slice)
            ),
            Err(HotkeyError::Unassigned)
        ));
    }

    #[test]
    fn caps_the_number_of_keys() {
        let keys: Vec<Vec<u8>> = (1..=6).map(dword_bytes).collect();

        let chord = Chord::decode(&dword_bytes(6), keys.iter().map(Vec::as_slice)).unwrap();

        assert_eq!(chord.codes().len(), MAX_KEYS);
    }
}
