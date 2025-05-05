create-examples-output:
    mkdir -p examples-output

[working-directory: 'examples-output']
example-spawn-streamed: create-examples-output
    cargo run -p rfr-subscriber --example spawn

[working-directory: 'examples-output']
viz-gen-spawn-streamed: create-examples-output
    cargo run -p rfr-viz -- generate recording-spawn-stream.rfr --name spawn-stream

spawn-streamed: example-spawn-streamed viz-gen-spawn-streamed

[working-directory: 'examples-output']
example-ping-pong-streamed: create-examples-output
    cargo run -p rfr-subscriber --example ping-pong

[working-directory: 'examples-output']
viz-gen-ping-pong-streamed: create-examples-output
    cargo run -p rfr-viz -- generate recording-ping_pong-stream.rfr --name ping-pong-stream

ping-pong-streamed: example-spawn-streamed viz-gen-spawn-streamed

[working-directory: 'examples-output']
example-spawn-chunked: create-examples-output
    cargo run -p rfr-subscriber --example spawn-chunked

[working-directory: 'examples-output']
viz-gen-spawn-chunked: create-examples-output
    cargo run -p rfr-viz -- generate chunked-spawn.rfr --name spawn-chunked

spawn-chunked: example-spawn-chunked viz-gen-spawn-chunked

[working-directory: 'examples-output']
example-ping-pong-chunked: create-examples-output
    cargo run -p rfr-subscriber --example ping-pong-chunked

[working-directory: 'examples-output']
viz-gen-ping-pong-chunked: create-examples-output
    cargo run -p rfr-viz -- generate chunked-ping-pong.rfr --name ping-pong-chunked

ping-pong-chunked: example-ping-pong-chunked viz-gen-ping-pong-chunked

[working-directory: 'examples-output']
example-barrier-chunked: create-examples-output
    cargo run -p rfr-subscriber --example barrier
    
[working-directory: 'examples-output']
viz-gen-barrier-chunked: create-examples-output
    cargo run -p rfr-viz -- generate chunked-barrier.rfr --name barrier
    
barrier-chunked: example-barrier-chunked viz-gen-barrier-chunked
    
[working-directory: 'examples-output']
example-thousand-tasks-chunked: create-examples-output
    cargo run -p rfr-subscriber --example thousand-tasks

[working-directory: 'examples-output']
viz-gen-thousand-tasks-chunked: create-examples-output
    cargo run -p rfr-viz -- generate chunked-thousand-tasks.rfr --name thousand-tasks

thousand-tasks-chunked: example-thousand-tasks-chunked viz-gen-thousand-tasks-chunked

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
