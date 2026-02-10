use clap::ValueEnum;

use crate::collect;
use crate::perfetto;

#[derive(Clone, Debug, ValueEnum)]
pub(crate) enum OutputFormat {
    Perfetto,
}

impl OutputFormat {
    pub(crate) fn extension(&self) -> &'static str {
        match self {
            OutputFormat::Perfetto => "pftrace",
        }
    }
}

pub(crate) fn convert(
    recording_path: &str,
    format: &OutputFormat,
    output_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let data = collect::collect_tasks(recording_path)?;

    match format {
        OutputFormat::Perfetto => perfetto::write_perfetto(&data, output_path)?,
    }

    Ok(())
}
