# Meta

The recording metadata contains configuration for a chunked recording.

## Format identifier

The chunked recording metadata file has the variant identifier `rfc-cm`. This chapter describes the
format for version `rfr-cm/0.0.1`.

For a description of the identifer encoding see the [Format identifier](format-identifier.md)
chapter.

## Structure

| Element            | Representation                       |
|--------------------|--------------------------------------|
| format\_identifier | [`string`] (see [Format Identifier]) |
| header             | [MetaHeader]                         |


## MetaHeader

The metadata header contains the initial creation time and a list of format identifiers for other
files in this recording.

| Element             | Representation                           |
|---------------------|------------------------------------------|
| created\_time       | [AbsTimestamp]                           |
| format\_identifiers | \[[`string`]\] (see [Format Identifier]) |

The format identifiers are all those that are used in this chunked recording. There will only be up
to one format identifier for each variant.

[Format Identifier]: #format-identifier

[MetaHeader]: #metaheader
[AbsTimestamp]: common.md#abstimestamp

[`string`]: https://postcard.jamesmunns.com/wire-format#15---string
