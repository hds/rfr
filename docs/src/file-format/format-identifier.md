# Format identifier

The format identifier is the beginning of a file, whether the streaming or chunked file format is
used. It declares the format variant and the version of that variant which is used.

The variant and version are encoded as a single [`string`] with the following parts:

- variant: 1 to 8 characters (all printable ASCII characters except `/`)
- divider: literal `/` character
- version: dot `.` separated version string containing decimal digit characters:
  - major
  - minor
  - patch

## Restrictions

The complete format identifier should occupy no more than 24 characters in total.


## Examples

The header string `rfr-foo/1.0.2` would decompose as:
- variant: `rfr-foo`
- divider: `/`
- version:
  - major: `1`
  - minor: `0`
  - patch: `2`


## Why is it stringly typed?

The format identifier is encoded as a string instead of using enum variant integers for the variant
and integers for the version parts. This is to allow easy human inspection of the file format at the
cost of slightly more complex parsing.

[`string`]: https://postcard.jamesmunns.com/wire-format#15---string
