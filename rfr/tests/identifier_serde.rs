use rfr::{FormatIdentifier, FormatVariant};

#[test]
fn rfr_s_roundtrip() {
    let version = FormatIdentifier {
        variant: FormatVariant::RfrStreaming,
        major: 0,
        minor: 1,
        patch: 2,
    };

    let buffer = postcard::to_stdvec(&version).unwrap();
    let deser_version: FormatIdentifier = postcard::from_bytes(buffer.as_slice()).unwrap();
    assert_eq!(version, deser_version);
}

#[test]
fn rfr_s_serialize() {
    let version = FormatIdentifier {
        variant: FormatVariant::RfrStreaming,
        major: 0,
        minor: 1,
        patch: 2,
    };

    let buffer = postcard::to_stdvec(&version).unwrap();
    let expected = postcard::to_stdvec("rfr-s/0.1.2").unwrap();

    assert_eq!(buffer, expected);
}

#[test]
fn rfr_s_deserialize() {
    let expected = FormatIdentifier {
        variant: FormatVariant::RfrStreaming,
        major: 0,
        minor: 1,
        patch: 2,
    };

    let buffer = postcard::to_stdvec("rfr-s/0.1.2").unwrap();
    let version: FormatIdentifier = postcard::from_bytes(buffer.as_slice()).unwrap();

    assert_eq!(version, expected);
}

#[test]
fn roundtrip_rfr_c_version() {
    let version = FormatIdentifier {
        variant: FormatVariant::RfrChunked,
        major: 3,
        minor: 4,
        patch: 652,
    };

    let buffer = postcard::to_stdvec(&version).unwrap();
    let deser_version: FormatIdentifier = postcard::from_bytes(buffer.as_slice()).unwrap();
    assert_eq!(version, deser_version);
}

#[test]
fn rfr_c_serialize() {
    let version = FormatIdentifier {
        variant: FormatVariant::RfrChunked,
        major: 3,
        minor: 4,
        patch: 652,
    };

    let buffer = postcard::to_stdvec(&version).unwrap();
    let expected = postcard::to_stdvec("rfr-c/3.4.652").unwrap();

    assert_eq!(buffer, expected);
}

#[test]
fn rfr_c_deserialize() {
    let expected = FormatIdentifier {
        variant: FormatVariant::RfrChunked,
        major: 3,
        minor: 4,
        patch: 652,
    };

    let buffer = postcard::to_stdvec("rfr-c/3.4.652").unwrap();
    let version: FormatIdentifier = postcard::from_bytes(buffer.as_slice()).unwrap();

    assert_eq!(version, expected);
}
