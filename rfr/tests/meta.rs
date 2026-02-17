// TODO(hds): Write tests for meta file handling
use rfr::{
    AbsTimestamp, FormatIdentifier, FormatVariant,
    chunked::{ChunkedMeta, ChunkedMetaHeader, MetaTryFromIoError},
};

#[test]
fn round_trip_meta() {
    let chunked_identifier = FormatIdentifier {
        variant: FormatVariant::RfrChunked,
        major: 3,
        minor: 4,
        patch: 652,
    };
    let meta = ChunkedMeta::new(vec![chunked_identifier.clone()]);

    let buffer = postcard::to_stdvec(&meta).unwrap();
    let deser_meta: ChunkedMeta = postcard::from_bytes(buffer.as_slice()).unwrap();

    assert_eq!(meta.format_identifier, deser_meta.format_identifier);
    assert_eq!(meta.header.created_time, deser_meta.header.created_time);
    assert_eq!(meta.header.format_identifiers, vec![chunked_identifier]);
}

// TODO(hds): also test with a method on ChunkedMeta that correctly checks the version in the meta
// file. Then check that we correctly identify when the version can't be read.

#[test]
fn try_from_io_success() {
    // If this test fails, maybe this identifier needs to be updated.
    let chunked_identifier = FormatIdentifier {
        variant: FormatVariant::RfrChunked,
        major: 3,
        minor: 4,
        patch: 652,
    };
    let meta = ChunkedMeta::new(vec![chunked_identifier.clone()]);

    let buffer = postcard::to_stdvec(&meta).unwrap();

    let read_meta = ChunkedMeta::try_from_io(buffer.as_slice()).unwrap();

    assert_eq!(meta.format_identifier, read_meta.format_identifier);
    assert_eq!(meta.header.created_time, read_meta.header.created_time);
    assert_eq!(meta.header.format_identifiers, vec![chunked_identifier]);
}

#[test]
fn try_from_io_invalid_format_identifier() {
    let buffer = [0_u8, 0_u8];
    let result = ChunkedMeta::try_from_io(buffer.as_slice());

    assert!(result.is_err());
    assert!(matches!(
        result,
        Err(MetaTryFromIoError::InvalidFormatIdentifier(_))
    ));
}

#[test]
fn try_from_io_incompatible_format() {
    let chunked_identifier = FormatIdentifier {
        variant: FormatVariant::RfrChunked,
        major: 3,
        minor: 4,
        patch: 652,
    };
    let meta = ChunkedMeta {
        format_identifier: FormatIdentifier {
            variant: FormatVariant::RfrChunkedMeta,
            major: 100,
            minor: 0,
            patch: 0,
        },
        header: ChunkedMetaHeader {
            created_time: AbsTimestamp::now(),
            format_identifiers: vec![chunked_identifier],
        },
    };

    let buffer = postcard::to_stdvec(&meta).unwrap();
    let result = ChunkedMeta::try_from_io(buffer.as_slice());

    assert!(result.is_err());
    assert!(matches!(
        result,
        Err(MetaTryFromIoError::IncompatibleFormat(FormatIdentifier {
            variant: FormatVariant::RfrChunkedMeta,
            ..
        }))
    ));
}

#[test]
fn try_from_io_file_invalid() {
    let format_identifier = FormatIdentifier {
        variant: FormatVariant::RfrChunkedMeta,
        major: 0,
        minor: 0,
        patch: 1,
    };

    let mut buffer = postcard::to_stdvec(&format_identifier).unwrap();
    buffer.append(&mut vec![0xff, 0xff]);
    let result = ChunkedMeta::try_from_io(buffer.as_slice());

    assert!(result.is_err());
    assert!(matches!(result, Err(MetaTryFromIoError::FileInvalid(_)),));
}

#[test]
fn try_from_io_missing_format_identifiers() {
    let meta = ChunkedMeta {
        format_identifier: FormatIdentifier {
            variant: FormatVariant::RfrChunkedMeta,
            major: 0,
            minor: 0,
            patch: 1,
        },
        header: ChunkedMetaHeader {
            created_time: AbsTimestamp::now(),
            format_identifiers: vec![],
        },
    };

    let buffer = postcard::to_stdvec(&meta).unwrap();

    let result = ChunkedMeta::try_from_io(buffer.as_slice());

    assert!(result.is_err());
    assert!(matches!(
        result,
        Err(MetaTryFromIoError::MissingFormatIdentifiers),
    ));
}
