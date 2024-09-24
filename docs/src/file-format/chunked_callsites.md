# Callsites

The callsites are stored in a separate file for the entire recording. Callsites are expected to be
static, so an append only list will be bounded by the total number of callsites in the instrumented
application.


## Format identifier

The chunked recording callsites file has the variant identifier `rfc-cc`. This chapter describes the
format for version `rfr-cc/0.0.1`.

For a description of the identifer encoding see the [Format identifier](format-identifier.md)
chapter.

## Structure

The callsite objects are stored as repeated elements until the end of the callsites file. These
objects will be added to during the execution of the instrumented application.

| Element            | Representation                       |
|--------------------|--------------------------------------|
| format\_identifier | [`string`] (see [Format Identifier]) |
| callsites          | [Callsite] (repeats)                 |

Typically, most callsites will be collected at the beginning of the recording, however further
callsites may be collected at any time.

[Format Identifier]: #format-identifier

[Callsite]: common.md#callsite
