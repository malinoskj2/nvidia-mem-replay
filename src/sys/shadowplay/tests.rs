use super::*;

#[test]
fn property_names_are_short_ascii_and_nul_padded() {
    let args = PropertyArgs::new(TEMPORARY_PATH).unwrap();

    assert_eq!(args.version, PROPERTY_ARGS_VERSION);
    assert_eq!(
        &args.name[..TEMPORARY_PATH.len()],
        TEMPORARY_PATH.as_bytes()
    );
    assert!(
        args.name[TEMPORARY_PATH.len()..]
            .iter()
            .all(|&byte| byte == 0)
    );
    assert_eq!(args.value.kind, 0);

    assert!(matches!(PropertyArgs::new(""), Err(ApiError::Name)));
    assert!(matches!(PropertyArgs::new("Tëmp"), Err(ApiError::Name)));
    assert!(matches!(
        PropertyArgs::new(&"x".repeat(PROPERTY_NAME_CAPACITY)),
        Err(ApiError::Name)
    ));
}

#[test]
fn only_bstr_variants_carry_text() {
    assert!(Variant::EMPTY.text().is_none());

    let null_bstr = Variant {
        kind: VT_BSTR,
        ..Variant::EMPTY
    };
    assert!(null_bstr.text().is_none());
}

#[test]
fn hresults_are_shown_as_unsigned_hex() {
    let error = ApiError::Call {
        name: TEMPORARY_PATH.to_owned(),
        result: 0x8007_0057_u32.cast_signed(),
    };

    assert!(error.to_string().contains("0x80070057"));
}

#[test]
fn only_failures_to_reach_the_engine_count_as_connection_failures() {
    let call = ApiError::Call {
        name: TEMPORARY_PATH.to_owned(),
        result: 0x8000_4005_u32.cast_signed(),
    };
    let load = ApiError::Load {
        path: PathBuf::from(LIBRARY),
        source: io::Error::from_raw_os_error(5),
    };
    for error in [call, load, ApiError::Create(-1)] {
        assert!(error.is_connection_failure(), "{error}");
    }

    for error in [
        ApiError::NotFound,
        ApiError::Export,
        ApiError::NoText(TEMPORARY_PATH.to_owned()),
        ApiError::Name,
        ApiError::Memory,
        ApiError::Lock,
    ] {
        assert!(!error.is_connection_failure(), "{error}");
    }
}
