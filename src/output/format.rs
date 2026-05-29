use clap::ValueEnum;

#[derive(Debug, Clone, Copy, PartialEq, ValueEnum)]
pub enum OutputFormatArg {
    Json,
    Table,
    Csv,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutputFormat {
    Json,
    Table,
    Csv,
}

/// Resolve the output format: explicit flag > TTY detection.
pub fn resolve(explicit: Option<OutputFormatArg>) -> OutputFormat {
    match explicit {
        Some(OutputFormatArg::Json) => OutputFormat::Json,
        Some(OutputFormatArg::Table) => OutputFormat::Table,
        Some(OutputFormatArg::Csv) => OutputFormat::Csv,
        None => {
            if is_terminal::is_terminal(std::io::stdout()) {
                OutputFormat::Table
            } else {
                OutputFormat::Json
            }
        }
    }
}
