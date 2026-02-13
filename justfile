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

[working-directory: 'examples-output']
example-long-chunked: create-examples-output
    cargo run -p rfr-subscriber --example long

[working-directory: 'examples-output']
viz-gen-long-chunked: create-examples-output
    cargo run -p rfr-viz -- generate chunked-long.rfr --name long

long-chunked: example-long-chunked viz-gen-long-chunked

[working-directory: 'examples-output']
example-outside-runtime-chunked: create-examples-output
    cargo run -p rfr-subscriber --example outside-runtime

[working-directory: 'examples-output']
viz-gen-outside-runtime-chunked: create-examples-output
    cargo run -p rfr-viz -- generate chunked-outside-runtime.rfr --name outside-runtime

outside-runtime-chunked: example-outside-runtime-chunked viz-gen-outside-runtime-chunked

all-examples: spawn-streamed ping-pong-streamed spawn-chunked ping-pong-chunked barrier-chunked thousand-tasks-chunked long-chunked outside-runtime-chunked

[working-directory: 'examples-output']
clean-all:
    rm -f barrier.html
    rm -rf chunked-barrier.rfr
    rm -rf chunked-ping-pong.rfr
    rm -rf chunked-spawn.rfr
    rm -rf chunked-thousand-tasks.rfr
    rm -rf chunked-long.rfr
    rm -rf chunked-outside-runtime.rfr
    rm -f ping-pong-chunked.html
    rm -f ping-pong-stream.html
    rm -f recording-ping_pong-stream.rfr
    rm -f recording-spawn-stream.rfr
    rm -f spawn-chunked.html
    rm -f spawn-stream.html
    rm -f thousand-tasks.html
    rm -f long.html
    rm -f outside-runtime.html
