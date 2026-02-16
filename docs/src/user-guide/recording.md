# Recording

RFR primarily focuses on recording async Rust programs which run on the [Tokio] async runtime. It
uses Tokio's built-in [Tracing] instrumentation.

[Tokio]: https://docs.rs/tokio/1/tokio/
[Tracing]: https://docs.rs/tracing/0.1/tracing/

## Step 1. Enable tracing in Tokio

You need to enable the `tracing` feature in Tokio. You want at least Tokio 1.21.0 to get the most of
the instrumentation, but using the latest version is better. Put the following in your `Cargo.toml`:

```toml
tokio = { version = "1", features = ["full", "tracing"] }
```

The `tracing` feature in Tokio is [unstable](https://docs.rs/tokio/1/tokio/#unstable-features), so
you also need to enable the `tokio_unstable` cfg flag. There are two ways of doing this, either use
`--cfg tokio_unstable` from the command line, or put the following into `.cargo/config.toml`:

```toml
[build]
rustflags = ["--cfg", "tokio_unstable"]
```

**Note**: the `[build]` section does **not** go in a `Cargo.toml` file. Instead it must be placed in
`.cargo/config.toml` within the root of your workspace. This is called a Cargo config file.

## Step 2. Initialize the RFR Tracing Layer

You need to initialize the `tracing-subscriber` registry with an RFR layer. The `RfrChunkedLayer`
should be used in almost all cases.

Add the following to your `Cargo.toml` to include the `tracing-subscriber` and `rfr-subscriber`
crates. Since `rfr-subscriber` hasn't been released to crates.io yet, it needs to be pulled from the
repository:

```toml
tracing-subscriber = { version = "0.3", features = [] }
rfr-subscriber = { git = "https://github.com/hds/rfr.git", ref = "main" }
```

Now, put the following code into your `main()` function.

```rust
fn main() {
    let rfr_layer = rfr_subscriber::RfrChunkedLayer::new("flight-recording.rfr");
    let flusher = rfr_layer.flusher();
    tracing_subscriber::registry()
        // .with(other_layer) 
        .with(rfr_layer)
        .init();

    // ...

    flusher.wait_flush().expect("Flushing flight recording failed");
}
```

The chunked layer takes the path to the recording that will be created. It will panic if there is
already something at that path. A chunked recording is a directory, so that is what you'll find
there once your program starts running.

Note that we're **not** using the `#[tokio::main]` attribute. Instead, we're going to set up the
Tokio runtime after configuring the Tracing subscriber. This is necessary so that information about
the start-up of the runtime is captured together with the instrumentation about the blocking tasks
that Tokio creates.

If you're creating other Tracing layers, then add them to the Registry before the call to `init()`.

## Step 3. Build and Start the Tokio Runtime

As mentioned above, we create the Tokio runtime "manually" so that we can collect all the
information about its start-up. This is really straight forward and you'll see the same thing if you
use the "expand macro" functionality of `rust-analyzer` on the `#[tokio::main]` attribute.

```rust
let rt = tokio::runtime::Builder::new_multi_thread()
    .enable_all()
    .build()
    .expect("Tokio runtime creation failed");

rt.block_on(real_main());
```

Here we construct a default multi-threaded Tokio runtime (see the [`runtime::Builder`] documentation
for more options). We then start the runtime passing it our "real" main function. This is any async
function (or you could pass an `async` block instead). You would have defined it like this:

```rust
async fn real_main() {
    // functionality here
}
```

[`runtime::Builder`]: https://docs.rs/tokio/1/tokio/runtime/struct.Builder.html

## Complete

That's it, now you can now run your progrma and you'll see a flight recording get created. Of
course, if you don't add any functionality, it won't be very interesting.

This is what your `main.rs` file should look like:

```rust
fn main() {
    let rfr_layer = rfr_subscriber::RfrChunkedLayer::new("flight-recording.rfr");
    let flusher = rfr_layer.flusher();
    tracing_subscriber::registry()
        .with(rfr_layer)
        .init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Tokio runtime creation failed");

    rt.block_on(real_main())

    flusher.wait_flush().expect("Flushing flight recording failed");
}

async fn real_main() {
    // functionality here
}
```
