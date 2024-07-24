# Common

A number of data structures are common between the [streaming](streaming.md) and
[chunked](chunked.md) file formats.


### Task

| Element    | Representation         |
|------------|------------------------|
| task\_id   | [TaskId]               |
| task\_name | [`string`]             |
| task\_kind | [TaskKind](#taskkind)  |
| context    | [`option`]\([TaskId]\) |


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


[TaskId]: #taskid

[tagged union]: https://postcard.jamesmunns.com/wire-format#tagged-unions
[`option`]: https://postcard.jamesmunns.com/wire-format#17---option
[`varint(u64)`]: https://postcard.jamesmunns.com/wire-format#10---u64
[`string`]: https://postcard.jamesmunns.com/wire-format#15---string
[`newtype_struct`]: https://postcard.jamesmunns.com/wire-format#21---newtype_struct
