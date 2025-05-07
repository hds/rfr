//! Chunked recording callsites
//!
//! The callsites are all stored centrally for the entire recording.
//!
//! See the [`ChunkedCallsites`] struct for details of the contents.

use std::{error, fmt, io};

use serde::{Deserialize, Serialize};

use crate::{
    chunked::WriteError,
    common::{Field, FieldName, Kind, Level},
    identifier::{FormatIdentifier, FormatVariant, ReadFormatIdentifierError},
};

/// The format identifier for the Callsites file
pub fn version() -> FormatIdentifier {
    FormatIdentifier {
        variant: FormatVariant::RfrChunkedCallsites,
        major: 0,
        minor: 0,
        patch: 1,
    }
}

/// An instrumented location in an application
///
/// A callsite contains all the const data for a specific location in the application where
/// instrumentation is emitted from.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Callsite {
    pub callsite_id: CallsiteId,
    pub level: Level,
    pub kind: Kind,
    pub const_fields: Vec<Field>,
    pub split_field_names: Vec<FieldName>,
}

/// The callsite Id defines a unique callsite.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub struct CallsiteId(u64);

impl From<u64> for CallsiteId {
    /// Create a CallsiteId from a `u64` value.
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl CallsiteId {
    /// The `u64` representation of the callsite Id.
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

/// Callsites file contents
///
/// This struct can be used to serialize and deserialize the chunked recording callsites file,
/// which is stored at `<chunked-recording.rfr>/callsites.rs`.
///
/// The callsite for all spans and events in the recording at stored centrally. This data is static
/// for the lifetime of a program execution and is expected to be bounded to a relatively small
/// number of items.
#[derive(Debug, Clone)]
pub struct ChunkedCallsites {
    /// Format identifier for the callsites file, the variant should be `rfr-cc`.
    pub format_identifier: FormatIdentifier,

    /// Meta file header.
    pub callsites: Vec<Callsite>,
}

impl ChunkedCallsites {
    /// Create a new callsites file contents with no callsites.
    pub fn new(callsites: Vec<Callsite>) -> Self {
        Self {
            format_identifier: version(),
            callsites,
        }
    }

    // Read from a chunked recording callsites file.
    //
    /// This method will attempt to load the contents of a chunked recording callsites file and
    /// return a [`ChunkedCallsites`] object.
    pub fn try_from_io(reader: impl io::Read) -> Result<Self, CallsitesTryFromIoError> {
        let mut reader = reader;

        let format_identifier = FormatIdentifier::try_from_io(&mut reader)
            .map_err(CallsitesTryFromIoError::InvalidFormatIdentifier)?;

        let current_version = version();
        if !current_version.can_read_version(&format_identifier) {
            return Err(CallsitesTryFromIoError::IncompatibleFormat(
                format_identifier,
            ));
        }

        let mut buffer = Vec::new();
        let _size = reader
            .read_to_end(&mut buffer)
            .map_err(CallsitesTryFromIoError::ReadFileFailed)?;

        let mut callsites = Vec::new();
        let mut bytes = buffer.as_slice();
        for idx in 0.. {
            if bytes.is_empty() {
                break;
            }

            let (callsite, rem_bytes): (Callsite, _) = postcard::take_from_bytes(bytes)
                .map_err(|error| CallsitesTryFromIoError::CallsiteInvalid { idx, error })?;
            bytes = rem_bytes;
            callsites.push(callsite);
        }

        Ok(ChunkedCallsites {
            format_identifier,
            callsites,
        })
    }

    /// Write this callsites file to the provided writer.
    ///
    /// This method writes the entire `callsites.rfr` file out.
    pub fn to_io(&self, writer: impl io::Write) -> Result<(), io::Error> {
        let mut writer = writer;
        postcard::to_io(&self.format_identifier, &mut writer)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

        for callsite in &self.callsites {
            postcard::to_io(callsite, &mut writer)
                .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        }

        Ok(())
    }
}

/// Incrementally write a chunked recording `callsites.rfr` file.
#[derive(Debug)]
pub struct ChunkedCallsitesWriter<W>
where
    W: io::Write,
{
    chunked_callsites: ChunkedCallsites,
    written_callsites: usize,
    writer: W,
}

impl<W> ChunkedCallsitesWriter<W>
where
    W: io::Write,
{
    /// Try to create a new chunked callsites writer.
    ///
    /// The provided writer will be kept for the lifetime of the chunked callsite writer and used
    /// to write callsites (added via [`push_callsite`]) during a [`flush`].
    ///
    /// # Errors
    ///
    /// This method will fail if the software defined format identifier cannot be written using the
    /// supplied writer.
    pub fn try_new(writer: W) -> Result<Self, WriteError> {
        let mut callsite_writer = Self {
            chunked_callsites: ChunkedCallsites::new(Vec::new()),
            written_callsites: 0,
            writer,
        };

        postcard::to_io(
            &callsite_writer.chunked_callsites.format_identifier,
            &mut callsite_writer.writer,
        )
        .map_err(WriteError::Serialization)?;

        Ok(callsite_writer)
    }

    /// Return a reference to the inner [`ChunkedCallsites`]
    pub fn chunked_callsites(&self) -> &ChunkedCallsites {
        &self.chunked_callsites
    }

    /// Push a callsite to be written during the next [`flush`].
    ///
    /// The list of existing callsites will be checked for duplicates
    pub fn push_callsite(&mut self, callsite: Callsite) -> PushCallsiteResult {
        for existing in &self.chunked_callsites.callsites {
            if existing.callsite_id == callsite.callsite_id {
                return PushCallsiteResult::Duplicate;
            }
        }
        self.chunked_callsites.callsites.push(callsite);
        PushCallsiteResult::Added
    }

    /// Returns whether there are unwritten callsites that need to be flushed to the writer.
    ///
    /// If this method returns `false`, then a call to [`flush`] will be a no-op.
    pub fn needs_flush(&mut self) -> bool {
        self.written_callsites == self.chunked_callsites.callsites.len()
    }

    /// Flushes any unwritten callsites to the writer.
    pub fn flush(&mut self) -> Result<(), FlushCallsitesError> {
        if self.written_callsites >= self.chunked_callsites.callsites.len() {
            // Nothing to flush
            return Ok(());
        }

        let to_write = &self.chunked_callsites.callsites[self.written_callsites..];
        for callsite in to_write {
            postcard::to_io(callsite, &mut self.writer).map_err(|inner| FlushCallsitesError {
                idx: self.written_callsites,
                inner,
            })?;
            self.written_callsites += 1;
        }

        Ok(())
    }
}

/// Result of pushing a callsite
#[derive(Debug)]
pub enum PushCallsiteResult {
    /// New callsite was added
    Added,

    /// A duplicate callsite (by `CallsiteId`) was found, the callsite wasn't pushed
    Duplicate,
}

/// An error that occurred while flushing callsites to writer.
#[derive(Debug)]
pub struct FlushCallsitesError {
    idx: usize,
    inner: postcard::Error,
}

impl fmt::Display for FlushCallsitesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to write callsite with index `{idx}`: {inner}",
            idx = self.idx,
            inner = self.inner,
        )
    }
}
impl error::Error for FlushCallsitesError {}

/// An error when reading a `callsites.rfr` file from a reader.
#[derive(Debug)]
pub enum CallsitesTryFromIoError {
    /// An underlying IO error when reading the file.
    ReadFileFailed(io::Error),
    /// The format identifier at the beginning of the file is malformed.
    InvalidFormatIdentifier(ReadFormatIdentifierError),
    /// The meta file is written in an incompatible format.
    IncompatibleFormat(FormatIdentifier),
    /// An invalid callsite was encountered.
    CallsiteInvalid { idx: usize, error: postcard::Error },
}
