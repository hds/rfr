# Streaming

The streaming format is designed to minimise in-process resource consumption for single threaded
applications. It can be implemented without keeping any central state between async events.

It can be implemented by a [Tracing] subscriber without the need for the [`tracing-subscriber`]
crate. It could be adapted to run in `!#[no_std]` environments.

It's primary downside is that a consumer must have the entire flight recording to interpret the
contents. As such, it is not suitable for long running applications unless post-processing of the
stream can be performed during execution.

## Format identifier

The streaming file format has the variant identifier `rfr-s`. This chapter describes the format for
version `rfr-s/0.0.3`.

For a description of the identifer encoding see the [Format identifier](format-identifier.md)
chapter.

## Structure

The RFR streaming format encodes information about a single execution of an application in a single
file. The structure is the following:

| Element            | Representation                       |
|--------------------|--------------------------------------|
| format\_identifier | [`string`] (see [Format Identifier]) |
| records            | [Record](#record) (repeats)          |
| end record         | [Record](#record)                    |

The records element is **not** a sequence (i.e. array or vector), but rather repeating elements.
When reading, care must be taken to respect the end of the file or stream that the flight recording
is being read from.

When the instrumented application terminates, a token record should indicate that no more records
will be written. This uses the `End` variant of the [Event](#event) stored in the final record.

### Record

A record contains timing metadata and a single event.

| Element | Representation  |
|---------|-----------------|
| meta    | [Meta](#meta)   |
| event   | [Event](#Event) |


### Meta

| Element   | Representation  |
|-----------|-----------------|
| timestamp | [AbsTimestamp]  |

### AbsTimestamp

An absolute timestamp measured as time since the UNIX epoch (`1970-01-01T00:00Z`). The time is
stored as seconds and sub-seconds as microsecond precision.

| Element        | Representation  |
|----------------|-----------------|
| secs           | [`varint(u64)`] |
| subsec\_micros | [`varint(u32)`] |


### Event

Event is a [tagged union] that contains objects and events concerning those objects.

| Variant        | Discriminant | Data                        |
|----------------|--------------|-----------------------------|
| Task           | 0            | [Task]                      |
| NewTask        | 1            | `id`: [TaskId]              |
| TaskPollStart  | 2            | `id`: [TaskId]              |
| TaskPollEnd    | 3            | `id`: [TaskId]              |
| TaskDrop       | 4            | `id`: [TaskId]              |
| WakerWake      | 5            | `waker`: [Waker]            |
| WakerWakeByRef | 6            | `waker`: [Waker]            |
| WakerClone     | 7            | `waker`: [Waker]            |
| WakerDrop      | 8            | `waker`: [Waker]            |
| End            | 9            |                             |


[AbsTimestamp]: #abstimestamp

[Task]: common.md#task
[TaskId]: common.md#taskid
[Waker]: common.md#waker

[tagged union]: https://postcard.jamesmunns.com/wire-format#tagged-unions
[`varint(u32)`]: https://postcard.jamesmunns.com/wire-format#9---u32
[`varint(u64)`]: https://postcard.jamesmunns.com/wire-format#10---u64
[`option`]: https://postcard.jamesmunns.com/wire-format#17---option
[`string`]: https://postcard.jamesmunns.com/wire-format#15---string
[`unit_variant`]: https://postcard.jamesmunns.com/wire-format#20---unit_variant
[`newtype_struct`]: https://postcard.jamesmunns.com/wire-format#21---newtype_struct
