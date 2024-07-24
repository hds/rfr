# Encoding

The contents of flight recording files is encoded in the Postcard wire format.

Postcard is a `#![no_std]` focused serializer and deserializer for [Serde].

Postcard aims to be convenient for developers in constrained environments, while allowing for
flexibility to customize behavior as needed.

The wire format specification is hosted at [postcard.jamesmunns.com]. It is implemented by the
[postcard crate].

[postcard crate]: https://docs.rs/postcard/1.0/postcard/
[postcard.jamesmunns.com]: https://postcard.jamesmunns.com/
[Serde]: https://serde.rs/
