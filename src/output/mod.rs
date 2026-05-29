pub mod csv;
pub mod format;
pub mod json;
pub mod table;

use format::OutputFormat;
use serde_json::Value;

/// Render a value in the specified output format.
pub fn render(value: &Value, format: OutputFormat) -> anyhow::Result<()> {
    match format {
        OutputFormat::Json => json::render(value),
        OutputFormat::Table => table::render(value),
        OutputFormat::Csv => csv::render(value),
    }
}
