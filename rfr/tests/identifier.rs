use std::str::FromStr;

use rfr::{FormatIdentifier, FormatVariant, ParseFormatVersionError};

#[test]
fn can_read_rfr_s_version() {
    let version = FormatIdentifier::from_str("rfr-s/0.1.2").unwrap();

    let expected = FormatIdentifier {
        variant: FormatVariant::RfrStreaming,
        major: 0,
        minor: 1,
        patch: 2,
    };

    assert_eq!(version, expected);
}

#[test]
fn can_read_rfr_c_version() {
    let version = FormatIdentifier::from_str("rfr-c/3.4.652").unwrap();

    let expected = FormatIdentifier {
        variant: FormatVariant::RfrChunked,
        major: 3,
        minor: 4,
        patch: 652,
    };

    assert_eq!(version, expected);
}

#[test]
fn not_enough_parts() {
    let error = FormatIdentifier::from_str("rfr-s-1.0.0").unwrap_err();

    let expected = ParseFormatVersionError::IncorrectParts;

    assert_eq!(error, expected);
}

#[test]
fn too_many_parts() {
    let error = FormatIdentifier::from_str("rfr-s/1/0/0").unwrap_err();

    let expected = ParseFormatVersionError::IncorrectParts;

    assert_eq!(error, expected);
}

#[test]
fn unknown_variant() {
    let error = FormatIdentifier::from_str("mog/1.0.0").unwrap_err();

    let expected = ParseFormatVersionError::UnknownVariant("mog".into());

    assert_eq!(error, expected);
}

#[test]
fn invalid_version() {
    let error = FormatIdentifier::from_str("rfr-c/a.0.0").unwrap_err();

    let expected = ParseFormatVersionError::InvalidVersion("a.0.0".into());

    assert_eq!(error, expected);
}
