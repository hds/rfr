use std::error;

use clap::{Parser, Subcommand};

mod collect;
mod generate;
mod ui;

use crate::{generate::generate_html, ui::start_ui};

#[derive(Parser)]
#[command(about, long_about = None)]
pub(crate) struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Generate HTML visualization
    Generate {
        /// The path to a flight recording file
        recording_file: String,

        #[arg(short, long)]
        name: String,
    },

    /// Open interactive UI
    Ui {
        /// The path to a flight recording file
        recording_file: String,
    },
}

fn main() -> Result<(), Box<dyn error::Error>> {
    let args = Args::parse();

    match args.command {
        Command::Generate {
            recording_file,
            name,
        } => generate_html(recording_file, name),
        Command::Ui { recording_file } => start_ui(recording_file)?,
    }

    Ok(())
}
