use super::*;

#[test]
fn binary_and_string_paths_preserve_the_registry_type() {
    for kind in [1, 2, 3] {
        let value = RawValue {
            kind,
            bytes: vec![],
        }
        .with_path(r"C:\Temp\影像");
        assert_eq!(value.path().unwrap(), r"C:\Temp\影像");
        assert_eq!(value.with_path(r"R:\NVIDIA-Replay").kind, kind);
    }
}

#[test]
fn rejects_malformed_or_empty_paths() {
    assert!(
        RawValue {
            kind: 3,
            bytes: vec![1]
        }
        .path()
        .is_err()
    );
    assert!(
        RawValue {
            kind: 1,
            bytes: vec![0, 0]
        }
        .path()
        .is_err()
    );
    assert!(
        RawValue {
            kind: 4,
            bytes: vec![65, 0]
        }
        .path()
        .is_err()
    );
}

#[test]
fn restoration_preserves_user_edits_and_original_binary_bytes() {
    let original = RawValue {
        kind: 3,
        bytes: vec![],
    }
    .with_path(r"C:\Temp");
    let replacement = original.with_path(r"R:\NVIDIA-Replay");
    let redirect = Redirect {
        original: original.clone(),
        replacement: replacement.clone(),
        original_path: r"C:\Temp".to_owned(),
        target: r"R:\NVIDIA-Replay".to_owned(),
    };
    assert_eq!(redirect.restore_value(&replacement), Some(&original));
    assert_eq!(
        redirect.restore_value(&original.with_path(r"D:\NewTemp")),
        None
    );
    assert_eq!(redirect.restore_value(&original), None);
}
