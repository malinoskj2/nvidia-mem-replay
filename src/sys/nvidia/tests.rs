use super::*;

#[test]
fn only_paths_on_the_ram_volume_count_as_inside_the_mount() {
    let target = r"C:\Users\me\AppData\Local\NvidiaMemReplay\ram\NVIDIA-Replay";
    assert!(inside_mount(target, target));
    assert!(inside_mount(
        r"c:\users\ME\appdata\local\nvidiamemreplay\RAM",
        target
    ));
    assert!(inside_mount(
        r"C:\Users\me\AppData\Local\NvidiaMemReplay\ram\other",
        target
    ));
    assert!(!inside_mount(r"C:\Users\me\Videos\temp", target));
    assert!(!inside_mount(
        r"C:\Users\me\AppData\Local\NvidiaMemReplay\ramdisk",
        target
    ));
    assert!(!inside_mount(r"C:\Users\me\AppData\Local", target));

    assert!(inside_mount(r"T:\anything", r"T:\NVIDIA-Replay"));
    assert!(!inside_mount(r"S:\NvidiaTemp", r"T:\NVIDIA-Replay"));
}

#[cfg(windows)]
#[test]
fn only_connection_failures_of_the_first_call_make_the_engine_unavailable() {
    use shadowplay::ApiError;

    let refused = || {
        NvidiaError::Api(ApiError::Call {
            name: shadowplay::TEMPORARY_PATH.to_owned(),
            result: -1,
        })
    };

    // Reading the live location first: a failed connection means the engine is not there.
    let unreachable = refused().while_reaching();
    assert!(matches!(unreachable, NvidiaError::Unreachable(_)));
    assert!(unreachable.is_engine_unavailable());
    assert!(
        unreachable
            .to_string()
            .contains("NVIDIA App is not running")
    );
    assert!(unreachable.to_string().contains("TempFilePath"));
    assert!(
        NvidiaError::Api(ApiError::Create(-1))
            .while_reaching()
            .is_engine_unavailable()
    );

    // The same HRESULT from a later call (apply, restore, the watchdog) is a real error.
    assert!(!refused().is_engine_unavailable());

    // Internal and value errors never count as an unreachable engine, whenever they happen.
    for error in [
        NvidiaError::Api(ApiError::Name),
        NvidiaError::Api(ApiError::Memory),
        NvidiaError::Api(ApiError::Lock),
        NvidiaError::Api(ApiError::NotFound),
        NvidiaError::Api(ApiError::NoText(shadowplay::TEMPORARY_PATH.to_owned())),
        NvidiaError::Path,
        NvidiaError::Format,
        NvidiaError::RamOriginal,
    ] {
        let message = error.to_string();
        let reinterpreted = error.while_reaching();
        assert!(!reinterpreted.is_engine_unavailable(), "{message}");
        assert_eq!(reinterpreted.to_string(), message);
    }
}

#[cfg(not(windows))]
#[test]
fn only_the_missing_platform_support_makes_the_engine_unavailable() {
    assert!(NvidiaError::Unsupported.is_engine_unavailable());
    assert!(
        NvidiaError::Unsupported
            .while_reaching()
            .is_engine_unavailable()
    );
    for error in [
        NvidiaError::Path,
        NvidiaError::Format,
        NvidiaError::RamOriginal,
    ] {
        assert!(!error.while_reaching().is_engine_unavailable());
    }
}

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

#[test]
fn live_restoration_only_replaces_this_apps_own_value() {
    let redirect = Redirect {
        original: RawValue {
            kind: 3,
            bytes: vec![],
        }
        .with_path(r"C:\Temp"),
        replacement: RawValue {
            kind: 3,
            bytes: vec![],
        }
        .with_path(r"R:\NVIDIA-Replay"),
        original_path: r"C:\Temp".to_owned(),
        target: r"R:\NVIDIA-Replay".to_owned(),
    };

    assert!(redirect.restores(r"R:\NVIDIA-Replay"));
    assert!(redirect.restores(r"r:\nvidia-replay"));
    assert!(!redirect.restores(r"D:\NewTemp"));
    assert!(!redirect.restores(r"C:\Temp"));
}
