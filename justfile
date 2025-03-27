create-examples-output:
    mkdir -p examples-output

[working-directory: 'examples-output']
spawn-streamed: create-examples-output
    cargo run -p rfr-subscriber --example spawn
    cargo run -p rfr-viz -- generate recording-spawn-stream.rfr --name spawn-stream

[working-directory: 'examples-output']
ping-pong-streamed: create-examples-output
    cargo run -p rfr-subscriber --example ping-pong
    cargo run -p rfr-viz -- generate recording-ping_pong-stream.rfr --name ping-pong-stream

[working-directory: 'examples-output']
spawn-chunked: create-examples-output
    cargo run -p rfr-subscriber --example spawn-chunked
    cargo run -p rfr-viz -- generate chunked-spawn.rfr --name spawn-chunked

[working-directory: 'examples-output']
ping-pong-chunked: create-examples-output
    cargo run -p rfr-subscriber --example ping-pong-chunked
    cargo run -p rfr-viz -- generate chunked-ping-pong.rfr --name ping-pong-chunked

[working-directory: 'examples-output']
barrier-chunked: create-examples-output
    cargo run -p rfr-subscriber --example barrier
    cargo run -p rfr-viz -- generate chunked-barrier.rfr --name barrier
    
[working-directory: 'examples-output']
thousand-tasks-chunked: create-examples-output
    cargo run -p rfr-subscriber --example thousand-tasks
    cargo run -p rfr-viz -- generate chunked-thousand-tasks.rfr --name thousand-tasks

all-examples: spawn-streamed ping-pong-streamed spawn-chunked ping-pong-chunked barrier-chunked thousand-tasks-chunked

[working-directory: 'examples-output']
clean-all:
    rm -f barrier.html
    rm -rf chunked-barrier.rfr
    rm -rf chunked-ping-pong.rfr
    rm -rf chunked-spawn.rfr
    rm -rf chunked-thousand-tasks.rfr
    rm -f ping-pong-chunked.html
    rm -f ping-pong-stream.html
    rm -f recording-ping_pong-stream.rfr
    rm -f recording-spawn-stream.rfr
    rm -f spawn-chunked.html
    rm -f spawn-stream.html
    rm -f thousand-tasks.html
