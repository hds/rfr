use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "xtask")]
#[command(about = "Build automation tasks for rfr workspace")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate Perfetto protobuf Rust code
    GenProtoPerfetto,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::GenProtoPerfetto => gen_proto_perfetto()?,
    }

    Ok(())
}

fn gen_proto_perfetto() -> Result<()> {
    let workspace_root = std::env::current_dir()?;
    let proto_dir = workspace_root.join("rfr-convert/proto");
    let out_dir = workspace_root.join("rfr-convert/src/generated");

    std::fs::create_dir_all(&out_dir)?;

    let proto_files = [
        "protos/perfetto/trace/trace.proto",
        "protos/perfetto/trace/trace_packet.proto",
        "protos/perfetto/trace/track_event/track_descriptor.proto",
        "protos/perfetto/trace/track_event/process_descriptor.proto",
        "protos/perfetto/trace/track_event/track_event.proto",
        "protos/perfetto/trace/track_event/debug_annotation.proto",
    ];

    let proto_paths: Vec<PathBuf> = proto_files.iter().map(|f| proto_dir.join(f)).collect();

    prost_build::Config::new()
        .out_dir(&out_dir)
        .compile_protos(&proto_paths, &[&proto_dir])?;

    Ok(())
}
