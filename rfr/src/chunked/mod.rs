use serde::{Deserialize, Serialize};

use crate::{
    AbsTimestamp, FormatIdentifier, FormatVariant,
    common::{Span, Task},
};

mod callsite;
mod meta;
mod read;
mod record;
mod sequence;
mod write;

pub use callsite::{
    Callsite, CallsiteId, ChunkedCallsites, ChunkedCallsitesWriter, FlushCallsitesError,
};
pub use meta::{ChunkedMeta, ChunkedMetaHeader, MetaTryFromIoError};
pub use read::{Recording, from_path};
pub use record::{Meta, Record, RecordData};
pub use sequence::{SeqChunk, SeqChunkBuffer, SeqChunkHeader, SeqId};
pub use write::{ChunkedWriter, NewChunkedWriterError, WaitForWriteError, WriteError};

fn current_software_version() -> FormatIdentifier {
    FormatIdentifier {
        variant: FormatVariant::RfrChunked,
        major: 0,
        minor: 0,
        patch: 2,
    }
}

/// A timestamp measured from the [`UNIX_EPOCH`].
///
/// This timestamp is absoluteand only contains the whole seconds. No sub-second component is
/// stored.
#[derive(Debug, Clone, Copy, Hash, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct AbsTimestampSecs {
    /// Whole seconds component of the timestamp, measured from the [`UNIX_EPOCH`].
    pub secs: u64,
}

impl From<AbsTimestamp> for AbsTimestampSecs {
    fn from(value: AbsTimestamp) -> Self {
        Self { secs: value.secs }
    }
}

impl AbsTimestampSecs {
    pub const ZERO: Self = Self { secs: 0 };

    pub fn as_micros(&self) -> u64 {
        self.secs * 1_000_000
    }
}

// A timestamp within a chunk.
//
// A chunk timestamp represents the time of a record with respect to the chunk's base time. It is
// stored as the number of microseconds since the base time. All records within a chunk must occur
// at the base time or afterwards.
#[derive(Clone, Copy, Debug, Hash, PartialEq, PartialOrd, Ord, Eq, Deserialize, Serialize)]
pub struct ChunkTimestamp {
    /// Microseconds since the chunk's base time
    pub micros: u64,
}

impl ChunkTimestamp {
    const ZERO: ChunkTimestamp = ChunkTimestamp { micros: 0 };

    pub fn new(micros: u64) -> Self {
        Self { micros }
    }

    /// Create a new chunk timestamp from a base time and an absolute timestamp.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rfr::{AbsTimestamp, chunked::{AbsTimestampSecs, ChunkTimestamp}};
    /// #
    /// let now = AbsTimestamp::now();
    /// let base_time = AbsTimestampSecs::from(now.clone());
    /// let chunk_ts = ChunkTimestamp::from_base_and_timestamp(base_time, &now);
    ///
    /// # let also_now = chunk_ts.to_abs_timestamp(base_time);
    /// # assert_eq!(now, also_now);
    /// ```
    pub fn from_base_and_timestamp(base_time: AbsTimestampSecs, timestamp: &AbsTimestamp) -> Self {
        let secs = timestamp.secs.saturating_sub(base_time.secs);
        let micros = (secs * 1_000_000) + timestamp.subsec_micros as u64;
        Self::new(micros)
    }

    /// Convert to an absolute timestamp, given the base timestamp for this chunk.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rfr::{AbsTimestamp, chunked::{AbsTimestampSecs, ChunkTimestamp}};
    /// #
    /// # let now = AbsTimestamp::now();
    /// # let base_time = AbsTimestampSecs::from(now.clone());
    /// let chunk_ts = ChunkTimestamp::from_base_and_timestamp(base_time, &now);
    ///
    /// let also_now = chunk_ts.to_abs_timestamp(base_time);
    /// # assert_eq!(now, also_now);
    /// ```
    pub fn to_abs_timestamp(&self, base_time: AbsTimestampSecs) -> AbsTimestamp {
        abs_timestamp(base_time, self)
    }
}

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub enum Object {
    Span(Span),
    Task(Task),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Chunk {
    header: ChunkHeader,
    seq_chunks: Vec<SeqChunk>,
}

impl Chunk {
    pub fn header(&self) -> &ChunkHeader {
        &self.header
    }

    pub fn seq_chunks(&self) -> &Vec<SeqChunk> {
        &self.seq_chunks
    }

    pub fn abs_timestamp(&self, chunk_timestamp: &ChunkTimestamp) -> AbsTimestamp {
        abs_timestamp(self.header.interval.base_time, chunk_timestamp)
    }
}

fn abs_timestamp(base_time: AbsTimestampSecs, chunk_timestamp: &ChunkTimestamp) -> AbsTimestamp {
    let chunk_timestamp_secs = chunk_timestamp.micros / 1_000_000;
    let chunk_timestamp_subsec_micros = (chunk_timestamp.micros % 1_000_000) as u32;

    AbsTimestamp {
        secs: base_time.secs + chunk_timestamp_secs,
        subsec_micros: chunk_timestamp_subsec_micros,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ChunkHeader {
    pub interval: ChunkInterval,

    pub earliest_timestamp: ChunkTimestamp,
    pub latest_timestamp: ChunkTimestamp,
}

impl ChunkHeader {
    fn new(interval: ChunkInterval) -> Self {
        let earliest_timestamp = interval.end_time;
        let latest_timestamp = interval.start_time;

        Self {
            interval,
            earliest_timestamp,
            latest_timestamp,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChunkInterval {
    base_time: AbsTimestampSecs,
    start_time: ChunkTimestamp,
    end_time: ChunkTimestamp,
}

impl ChunkInterval {
    pub fn from_timestamp_and_period(timestamp: AbsTimestamp, period_micros: u64) -> Self {
        let (base_time, start_time) = if period_micros > 1_000_000 {
            let secs = AbsTimestampSecs::from(timestamp.clone());
            (
                AbsTimestampSecs {
                    secs: secs.secs - (secs.secs % (period_micros / 1_000_000)),
                },
                // Since the period is in whole seconds, the start offset is always 0.
                ChunkTimestamp::ZERO,
            )
        } else {
            (
                AbsTimestampSecs::from(timestamp.clone()),
                // Calculate the start time (offset) based on the period.
                ChunkTimestamp::new(
                    (timestamp.subsec_micros - (timestamp.subsec_micros % period_micros as u32))
                        as u64,
                ),
            )
        };

        let end_time = ChunkTimestamp::new(start_time.micros + period_micros);

        Self {
            base_time,
            start_time,
            end_time,
        }
    }

    /// The start time of the interval as an absolute timestamp
    ///
    /// # Examples
    ///
    /// ```
    /// # use rfr::{AbsTimestamp, chunked::ChunkInterval};
    /// let now = AbsTimestamp::now();
    /// let interval = ChunkInterval::from_timestamp_and_period(now.clone(), 1_000_000);
    ///
    /// let start_time = interval.abs_start_time();
    /// assert!(start_time <= now);
    /// ```
    pub fn abs_start_time(&self) -> AbsTimestamp {
        self.start_time.to_abs_timestamp(self.base_time)
    }

    /// The end time of the interval as an absolute timestamp
    ///
    /// # Examples
    ///
    /// ```
    /// # use rfr::{AbsTimestamp, chunked::ChunkInterval};
    /// let now = AbsTimestamp::now();
    /// let interval = ChunkInterval::from_timestamp_and_period(now.clone(), 1_000_000);
    ///
    /// let end_time = interval.abs_end_time();
    /// assert!(end_time >= now);
    /// ```
    pub fn abs_end_time(&self) -> AbsTimestamp {
        self.end_time.to_abs_timestamp(self.base_time)
    }
}
