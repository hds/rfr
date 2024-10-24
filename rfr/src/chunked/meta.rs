//! Chunked recording metadata
//!
//! The recording metadata contains configuration for a chunked recording.
//!
//! See the [`ChunkedMeta`] struct for details of the contents.

use std::io;

use serde::{Deserialize, Serialize};

use crate::{identifier::ReadFormatIdentifierError, rec, FormatIdentifier, FormatVariant};

/// The format identifier for the Meta file
pub fn version() -> FormatIdentifier {
    FormatIdentifier {
        variant: FormatVariant::RfrChunkedMeta,
        major: 0,
        minor: 0,
        patch: 1,
    }
}

/// Meta file contents
///
/// This struct can be used to serialize and deserialize the chunked recording meta file which is
/// stored at `<chunked-recording.rfr>/meta.rfr`.
///
/// The metadata for a recording includes the time that the recording was created. This time should
/// not be after the creation time of any chunks in the recording, but is otherwise only present
/// for user reference.
///
/// There is also a list of format identifiers which may be used in the recording. Software that is
/// going to read a recording can check that it is able to read all parts of the recording before
/// beginning.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChunkedMeta {
    /// Format identifier for the meta file, the variant should be `rfr-cm`.
    pub format_identifier: FormatIdentifier,

    /// Meta file header.
    pub header: ChunkedMetaHeader,
}

impl ChunkedMeta {
    /// Create new meta file contents with the provided format identifiers.
    ///
    /// The creation time will be set to the current time.
    ///
    /// # Panics
    ///
    /// Panics if the `format_identifiers` vector is empty.
    pub fn new(format_identifiers: Vec<FormatIdentifier>) -> Self {
        assert!(
            !format_identifiers.is_empty(),
            "at least the `rfr-c` format identifier must be supplied"
        );

        Self {
            format_identifier: version(),
            header: ChunkedMetaHeader {
                created_time: rec::AbsTimestamp::now(),
                format_identifiers,
            },
        }
    }

    /// Read from a recording meta file.
    ///
    /// This method will attempt to load the contents of a meta recording file and return a Meta
    /// object.
    pub fn try_from_io(reader: impl io::Read) -> Result<Self, MetaTryFromIoError> {
        let mut reader = reader;

        let format_identifier = FormatIdentifier::try_from_io(&mut reader)
            .map_err(MetaTryFromIoError::InvalidFormatIdentifier)?;

        let current_version = version();
        if !current_version.can_read_version(&format_identifier) {
            return Err(MetaTryFromIoError::IncompatibleFormat(format_identifier));
        }

        let mut buffer = Vec::new();
        let _size = reader
            .read_to_end(&mut buffer)
            .map_err(MetaTryFromIoError::ReadFileFailed)?;

        let header: ChunkedMetaHeader =
            postcard::from_bytes(buffer.as_slice()).map_err(MetaTryFromIoError::FileInvalid)?;

        // TODO(hds): We should check that the necessary format identifiers are present. Right now,
        // that means `rfr-c`.
        if header.format_identifiers.is_empty() {
            return Err(MetaTryFromIoError::MissingFormatIdentifiers);
        }

        Ok(ChunkedMeta {
            format_identifier,
            header,
        })
    }
}

/// An error returned when attempting to read a recording meta file.
#[derive(Debug)]
pub enum MetaTryFromIoError {
    /// An underlying IO error when reading the file.
    ReadFileFailed(io::Error),
    /// The format identifier at the beginning of the file is malformed.
    InvalidFormatIdentifier(ReadFormatIdentifierError),
    /// The meta file is written in an incompatible format.
    IncompatibleFormat(FormatIdentifier),
    /// The file is not a valid Meta object serialized to [Postcard].
    ///
    /// [Postcard]: crate@postcard
    FileInvalid(postcard::Error),
    /// The meta file contains no format identifiers indicating the format of the remaining files
    /// in the recording.
    MissingFormatIdentifiers,
}

/// Header for the chunked recording meta file.
///
/// See [`ChunkedMeta`] for more details and usage.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChunkedMetaHeader {
    /// The time that this recording was created
    pub created_time: rec::AbsTimestamp,

    /// All the format identifiers used in this chunked recording
    ///
    /// Only one format identifier for each variant should be included.
    pub format_identifiers: Vec<FormatIdentifier>,
}
