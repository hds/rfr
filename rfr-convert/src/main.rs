use std::error;

use clap::Parser;

mod collect;
mod convert;
mod generated;
mod perfetto;

use crate::convert::{convert, OutputFormat};

#[derive(Parser)]
#[command(about = "Convert rfr recordings to other trace formats", long_about = None)]
struct Args {
    /// The path to a chunked rfr recording directory
    recording_path: String,

    /// Output format
    #[arg(short, long)]
    format: OutputFormat,

    /// Output file path (defaults to <input_stem>.<format_extension>)
    #[arg(short, long)]
    output: Option<String>,
}

fn main() -> Result<(), Box<dyn error::Error>> {
    let args = Args::parse();

    let output_path = args.output.unwrap_or_else(|| {
        let stem = std::path::Path::new(&args.recording_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        format!("{}.{}", stem, args.format.extension())
    });

    convert(&args.recording_path, &args.format, &output_path)?;

    Ok(())
}
