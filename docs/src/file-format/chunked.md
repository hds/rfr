# Chunked

The chunked format is designed to balance in-process resource consumption for multi-threaded
applications with reduced storage requirements and the possibility of reading only small sections of
the flight recording at a time without having to have previously consumed all prior sections.

## Format identifier

The chunked file format has the variant identifier `rfr-c`. This chapter describes the format for
version `rfr-c/0.0.3`.

For a description of the identifer encoding see the [Format identifier](format-identifier.md)
chapter.

## Recording Structure

The RFR chunked format encodes information about a singe execution of an application in multiple
files - "chunks". Each chunk represents a specific, non-overlapping time period during the
application execution.

The layout of the separate files on disk is the following:

- dir: `<recording-name>.rfr/`
  - file: `meta.rfr`
  - file: `callsites.rfr`
  - dir: `<year>-<month>/<day>-<hour>/`
    - file: `chunk-<minute>-<second>.rfr`

The exact split of sub-directories is not overly important, it's there to help humans navigate the
flight recording structure and to avoid placing too many files in a single directory, which is
something that some file systems have problems with.

The key take away is htat we have reserved top level files for flight recording wide information.

- `meta.rfr` - recording configuration. See the [Meta](chunked_meta.md) chapter for details.
- `callsites.rs` - append only list of callsites. See the [Callsites](chunked_callsites.md) chapter
  for details.

The remaining files are each self-contained recording files for a short time period, on the order of
1 second.

## Chunk Structure

The chunk file encodes information about a short period of time during the execution of a single
application. The structure tries to make the file as independent as possible, so that chunks can be
read from the middle of an execution. The structure is the following:

| Element            | Representation                       |
|--------------------|--------------------------------------|
| format\_identifier | [`string`] (see [Format Identifier]) |
| header             | [ChunkHeader]                        |
| seq\_chunks        | \[[SeqChunk]\]                       |

### ChunkHeader

The chunk header contains metadata for the chunk.

| Element             | Representation     |
|---------------------|--------------------|
| interval            | [ChunkInterval]   |
| earliest\_timestamp | [ChunkTimestamp]   |
| latest\_timestamp   | [ChunkTimestamp]   |

The earliest timestamp and latest timestamp are the minimum and the maximum of the same value in the
[SeqChunkHeader] for all the sequence chunks that make up this chunk. As such, they are the minimum
and maximum times of the records in this chunk as a whole.

These timestamps are relative to the base time in the interval.

### ChunkInterval

A chunk interval describes the period of time that a chunk represents. Only a single chunk is
responsible for any given instant in time.

The time period a chunk interval represents should either be a whole number of seconds or it should
divide 1 second without remainder.

| Element             | Representation     |
|---------------------|--------------------|
| base\_time          | [AbsTimestampSecs] |
| start\_time         | [ChunkTimestamp]   |
| end\_time           | [ChunkTimestamp]   |

The base time is the absolute time which is used as a reference for all chunk internal times. The
base time has seconds precision, so all chunk timestamps will start on a second boundary.

The remaining times in the interval are chunk timestamps and so are relative to the base time.

The start and end times represent the time period that a chunk covers within the recording. All
records that occur on or after the start time and strictly before the end time will be included in
this chunk and not in any other chunk.

Note that while a chunk's base time is always measured in whole seconds, the chunk may have start
and end times which aren't, for example if the chunk period is less than 1 second.

### AbsTimestampSecs

An absolute timestamp measured as time since the UNIX epoch (`1970-01-01T00:00Z`). The time has
seconds resolution and is encoded as a [`varint(u64)`].

### ChunkTimestamp

A chunk timestamp represents the time of a record with respect to the chunk's base time. It is
stored as the number of microseconds since the base time. All records within a chunk must occur at
the base time or afterwards.

Chunk timestamps are encoded as a [`newtype_struct`] of a [`varint(u64)`]. This gives it a range of
500 thousand years after the base time, which is more than enough.

<div class="warning">
NEEDS REVIEW:

At microsecond precision, a `u32` would only provide 71 minutes of range, which may be enough,
should we switch to storing a `u32` internally? These values are encoded as [`varint(u64)`] so there
won't be any difference in the file format as long as we stay within the value range of a `u32`,
just the internal memory representation.
</div>

### SeqChunk

Records in a single [Chunk] may not be globally ordered. However, all records in a sequence chunk
must be present in order. Generally this corresponds to records emitted from a single thread. After
a chunk's time period has finished, all the sequences are collected and written out. The sequence
chunk would then contain the thread local recording.

Sequence chunks are not aggregated prior to being written out. As such, it is possible that the
objects stored in one sequence chunk may be duplicated in other sequence chunks within the same
parent chunk.

| Element     | Representation    |
|-------------|-------------------|
| header      | [SeqChunkHeader]  |
| objects     | \[[Object]\]      |
| records     | \[[Record]\]      |

The objects array contains all objects referenced by records in this sequence chunk. The records
contain the occurences during the time period. This structure is different from the [streaming] file
format where records and objects are mixed in a single stream.

### SeqChunkHeader

Header information for a sequence chunk.

| Element             | Representation    |
|---------------------|-------------------|
| seq\_id             | [SeqId]           |
| earliest\_timestamp | [ChunkTimestamp]  |
| latest\_timestamp   | [ChunkTimestamp]  |

The earliest timestamp and latest timestamp are the minimum and maximum times of the records in this
sequence chunk respectively.

### SeqId

The sequence identifier is an internal identifier for an in-order sequence of records created by
instrumentation. Usually, a sequence identifier maps directly to a thread where instrumentation is
collected. It is stored as a [`newtype_struct`] of a single [`varint(u64)`].

### Object

An object is a [tagged union] that contains object data. Object data isn't expected to change
significanly during the course of an application execution.

At this time, the only objects are tasks.

| Variant | Discriminant | Data    |
|---------|--------------|---------|
| Span    | 0            | [Span]  |
| Task    | 1            | [Task]  |

### Record

A record contains timing metadata and record data.

| Element | Representation |
|---------|----------------|
| meta    | [Meta]         |
| data    | [RecordData]   |


### Meta

Metadata for a record.

| Element   | Representation   |
|-----------|------------------|
| timestamp | [ChunkTimestamp] |

### RecordData

A record is a [tagged union] that contains information about an occurence in the instrumented
application.

| Variant        | Discriminant | Data                       |
|----------------|--------------|----------------------------|
| SpanNew        | 0            | `iid`: [InstrumentationId] |
| SpanEnter      | 1            | `iid`: [InstrumentationId] |
| SpanExit       | 2            | `iid`: [InstrumentationId] |
| SpanClose      | 3            | `iid`: [InstrumentationId] |
| Event          | 4            | `event`: [Event]           |
| NewTask        | 5            | `iid`: [InstrumentationId] |
| TaskPollStart  | 6            | `iid`: [InstrumentationId] |
| TaskPollEnd    | 7            | `iid`: [InstrumentationId] |
| TaskDrop       | 8            | `iid`: [InstrumentationId] |
| WakerWake      | 9            | `waker`: [Waker]           |
| WakerWakeByRef | 10           | `waker`: [Waker]           |
| WakerClone     | 11           | `waker`: [Waker]           |
| WakerDrop      | 12           | `waker`: [Waker]           |

Records are encoded in a single large [tagged union] rather than hierachically as each level of a
union hierarchy costs an extra byte (for unions with up to 127 variants).


[Format Identifier]: #format-identifier

[AbsTimestampSecs]: #abstimestampsecs
[ChunkHeader]: #chunkheader
[ChunkInterval]: #chunkinterval
[ChunkTimestamp]: #chunktimestamp
[Record]: #record
[RecordData]: #recorddata
[Meta]: #meta
[Object]: #object
[SeqChunk]: #seqchunk
[SeqChunkHeader]: #seqchunkheader
[SeqId]: #seqid

[InstrumentationId]: common.md#instrumentationid
[Span]: common.md#span
[Event]: common.md#event
[Task]: common.md#task
[TaskId]: common.md#taskid
[Waker]: common.md#waker
[streaming]: streaming.md

[tagged union]: https://postcard.jamesmunns.com/wire-format#tagged-unions
[`option`]: https://postcard.jamesmunns.com/wire-format#17---option
[`varint(u64)`]: https://postcard.jamesmunns.com/wire-format#10---u64
[`string`]: https://postcard.jamesmunns.com/wire-format#15---string
[`newtype_struct`]: https://postcard.jamesmunns.com/wire-format#21---newtype_struct
