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
| split\_field\_names | \[[FieldName]\]              |

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
runtime are stored in `split_field_names`.

### CallsiteId

The callsite Id defines a unique callsite. It is stored as a [`newtype_struct`] of a single
[`varint(u64)`].

### Level

The level at which a span or event is recorded. Levels are tied to the callsite, so it is static
information.

The level is stored as a [`varint(u8)`]. The `tracing` levels are mapped as per the [Bunyan level
suggestions](https://github.com/trentm/node-bunyan?tab=readme-ov-file#levels).

| Value | Level |
|-------|-------|
| 10    | trace |
| 20    | debug |
| 30    | info  |
| 40    | warn  |
| 50    | error |

### Kind

The kind of a callsite indicates the type of the instrumentation that is emitted.

| Variant        | Discriminant | Data |
|----------------|--------------|------|
| Unknown        | 0            |      |
| Event          | 1            |      |
| Span           | 2            |      |

### Field

A complete field containing the name and value together.

| Element | Representation |
|---------|----------------|
| name    | [FieldName]    |
| value   | [FieldValue]   |

### FieldName

The name of a field is represented by a [`string`].

### FieldValue

The value of a field. Only "basic" values are made available.

| Variant | Discriminant | Data             |
|---------|--------------|------------------|
| F64     | 0            | [`f64`]          |
| I64     | 1            | [`varint(i64)`]  |
| U64     | 2            | [`varint(u64)`]  |
| I128    | 3            | [`varint(i128)`] |
| U128    | 4            | [`varint(i128)`] |
| Bool    | 5            | [`bool`]         |
| Str     | 6            | [`string`]       |

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
| split\_field\_values | \[[FieldValue]\]       |
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
| split\_field\_values | \[[FieldValue]\]       |
| dynamic\_fields      | \[[Field]\]            |


### Parent

| Variant        | Discriminant | Data                       |
|----------------|--------------|----------------------------|
| Current        | 0            |                            |
| Root           | 1            |                            |
| Explicit       | 2            | `iid`: [InstrumentationId] |

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
[Level]: #level
[Kind]: #kind
[Field]: #field
[FieldName]: #fieldname
[FieldValue]: #fieldvalue
[Parent]: #parent
[TaskId]: #taskid
[TaskId]: #taskid

[tagged union]: https://postcard.jamesmunns.com/wire-format#tagged-unions
[`bool`]: https://postcard.jamesmunns.com/wire-format#1---bool
[`f64`]: https://postcard.jamesmunns.com/wire-format#13---f64
[`option`]: https://postcard.jamesmunns.com/wire-format#17---option
[`varint(u8)`]: https://postcard.jamesmunns.com/wire-format#7---u8
[`varint(u32)`]: https://postcard.jamesmunns.com/wire-format#9---u32
[`varint(i64)`]: https://postcard.jamesmunns.com/wire-format#5---i64
[`varint(u64)`]: https://postcard.jamesmunns.com/wire-format#10---u64
[`varint(i128)`]: https://postcard.jamesmunns.com/wire-format#6---i128
[`varint(u128)`]: https://postcard.jamesmunns.com/wire-format#11---u128
[`string`]: https://postcard.jamesmunns.com/wire-format#15---string
[`newtype_struct`]: https://postcard.jamesmunns.com/wire-format#21---newtype_struct
