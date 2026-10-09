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
