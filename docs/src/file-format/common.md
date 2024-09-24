# Common

A number of data structures are common between the [streaming](streaming.md) and
[chunked](chunked.md) file formats.

## Time

### AbsTimestamp

An absolute timestamp measured as time since the UNIX epoch (`1970-01-01T00:00Z`). The time is
stored as seconds and sub-seconds as microsecond precision.

| Element        | Representation  |
|----------------|-----------------|
| secs           | [`varint(u64)`] |
| subsec\_micros | [`varint(u32)`] |


## Generic objects and actions

### Callsite

A callsite represents the location in the instrumented application's source (code) where
instrumentation is emitted from.

| Element             | Representation               |
|---------------------|------------------------------|
| callsite\_id        | [CallsiteId]                 |
| level               | [Level]                      |
| kind                | [Kind]                       |
| const\_fields       | \[[Field]\]                  |
| const\_field\_names | \[[FieldName]\]              |

A callsite contains const data for a span, event, or other object. This data will not change during
the runtime of the instrumented.

A callsite maps to the Metadata struct from `tracing`, the following fields on the struct are stored
as `const_fields`:
- `name`
- `target`
- `module_path`
- `file`
- `line`

The remaining fields whose names, but not values are constant for the instrumented application's
runtime are stored in `const_field_names`.

### CallsiteId

The callsite Id defines a unique callsite. It is stored as a [`newtype_struct`] of a single
[`varint(u64)`].

### Span

A span represents a period of time, within which the span may change from active to inactive
multiple times during its lifetime.

A span is a generic object which can be stored based on available information and later interpreted
by the reader of a flight recording.

| Element              | Representation         |
|----------------------|------------------------|
| iid                  | [InstrumentationId]    |
| callsite\_id         | [CallsiteId]           |
| parent               | [Parent]               |
| const\_field\_values | \[[FieldValue]\]       |
| dynamic\_fields      | \[[Field]\]            |

### InstrumentationId

The instrumentation Id is the instrumentation defined identifier for an object stored as a
[`newtype_struct`] of a single [`varint(u64)`].

This Id is used to link actions (FIXME(hds): link to sections for streamed and chunked) to the
objects (FIXME(hds): link to sections for streamed and chunked) that they are affecting.

When using tracing to generate the instrumentation, tracing's
[`span::Id`](https://docs.rs/tracing/0.1/tracing/span/struct.Id.html) can be used for the
instrumentation Id.


### Event

An event represents a moment in time.

An event is a generic action which can be stored with the available information and interpreted by
the reader of the flight recording.

| Element              | Representation         |
|----------------------|------------------------|
| callsite\_id         | [CallsiteId]           |
| parent               | [Parent]               |
| const\_field\_values | \[[FieldValue]\]       |
| dynamic\_fields      | \[[Field]\]            |


### Parent

| Variant        | Discriminant | Data                       |
|----------------|--------------|----------------------------|
| Current        | 0            |                            |
| Root           | 1            |                            |
| Explicit       | 2            | `iid`: [InstrumentationId] |

### FieldName

### Task

| Element      | Representation         |
|--------------|------------------------|
| iid          | [InstrumentationId]    |
| callsite\_id | [CallsiteId]           |
| task\_id     | [TaskId]               |
| task\_name   | [`string`]             |
| task\_kind   | [TaskKind](#taskkind)  |
| context      | [`option`]\([TaskId]\) |


### TaskId

The task Id is the runtime defined identifier for a task stored as a [`newtype_struct`] of a single
[`varint(u64)`].


### TaskKind

The kind of a task is stored as [tagged union], however only the `Other` variant has additional
data.

| Variant  | Discriminant | Data       |
|----------|--------------|------------|
| Task     | 0            |            |
| Local    | 1            |            |
| Blocking | 2            |            |
| BlockOn  | 3            |            |
| Other    | 4            | [`string`] |


### Waker

A waker action contains context information about the waker and where the action occurred.

The task Id is that of the task that the waker will wake when invoked. The context describes where
the waker action occurred, specifically when it occurred within a task.

| Element    | Representation         |
|------------|------------------------|
| task\_id   | [TaskId]               |
| context    | [`option`]\([TaskId]\) |


[InstrumentationId]: #instrumentationid
[CallsiteId]: #callsiteid
[Parent]: #parent
[TaskId]: #taskid
[TaskId]: #taskid

[tagged union]: https://postcard.jamesmunns.com/wire-format#tagged-unions
[`option`]: https://postcard.jamesmunns.com/wire-format#17---option
[`varint(u64)`]: https://postcard.jamesmunns.com/wire-format#10---u64
[`string`]: https://postcard.jamesmunns.com/wire-format#15---string
[`newtype_struct`]: https://postcard.jamesmunns.com/wire-format#21---newtype_struct
