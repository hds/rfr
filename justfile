create-examples-output:
    mkdir -p examples-output

[working-directory: 'examples-output']
spawn-streamed: create-examples-output
    cargo run -p rfr-subscriber --example spawn
    cargo run -p rfr-viz -- recording-spawn-stream.rfr --name spawn-stream

[working-directory: 'examples-output']
ping-pong-streamed: create-examples-output
    cargo run -p rfr-subscriber --example ping-pong
    cargo run -p rfr-viz -- recording-ping_pong-stream.rfr --name ping-pong-stream

[working-directory: 'examples-output']
spawn-chunked: create-examples-output
    cargo run -p rfr-subscriber --example spawn-chunked
    cargo run -p rfr-viz -- chunked-spawn.rfr --name spawn-chunked

[working-directory: 'examples-output']
ping-pong-chunked: create-examples-output
    cargo run -p rfr-subscriber --example ping-pong-chunked
    cargo run -p rfr-viz -- chunked-ping-pong.rfr --name ping-pong-chunked

[working-directory: 'examples-output']
barrier-chunked: create-examples-output
    cargo run -p rfr-subscriber --example barrier
    cargo run -p rfr-viz -- chunked-barrier.rfr --name barrier

all-examples: spawn-streamed ping-pong-streamed spawn-chunked ping-pong-chunked barrier-chunked

[working-directory: 'examples-output']
clean-all:
    rm -f barrier.html
    rm -rf chunked-barrier.rfr
    rm -rf chunked-ping-pong.rfr
    rm -rf chunked-spawn.rfr
    rm -f ping-pong-chunked.html
    rm -f ping-pong-stream.html
    rm -f recording-ping_pong-stream.rfr
    rm -f recording-spawn-stream.rfr
    rm -f spawn-chunked.html
    rm -f spawn-stream.html
