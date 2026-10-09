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
