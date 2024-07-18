# File Format

[RFR](glossary.md#rfr) defines formats for storing flight recordings based on different
requirements.

The [streaming](file-format/streaming.md) format is designed to minimise in-process resource
consumption for single threaded applications. It can be implemented by a [Tracing] subscriber
without the [`Registry`] or any additional state and could be adapted to run in `no_std` environments.
It's primary downside is that a consumer must have the entire flight recording to interpret the
contents. As such, it is not suitable for long running applications unless post-processing of the
stream can be performed during execution.

The [chunked](file-format/chunked.md) format is designed to balance in-process resource consumption
for multi-threaded applications with reduced storage requirements and the possibility of reading
only small sections of the flight recording at a time without having to have previously consumed all
prior sections.

Both formats have a common [header](file-format/header.md) and use the [Postcard] wire format for
the [encoding](file-format/encoding.md).

[Postcard]: https://postcard.jamesmunns.com/
[`Registry`]: https://docs.rs/tracing-subscriber/0.3/tracing_subscriber/registry/struct.Registry.html
[Tracing]: https://docs.rs/tracing
