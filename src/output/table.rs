use comfy_table::{presets::UTF8_FULL_CONDENSED, ContentArrangement, Table};
use serde_json::Value;

/// Render a JSON value as a human-readable table.
pub fn render(value: &Value) -> anyhow::Result<()> {
    match value {
        Value::Array(arr) if !arr.is_empty() => render_array(arr),
        Value::Object(obj) => render_object(obj),
        Value::String(s) => {
            println!("{s}");
            Ok(())
        }
        _ => {
            // Fallback to JSON for non-tabular data
            super::json::render(value)
        }
    }
}

/// Render an array of objects as a table with column headers.
fn render_array(arr: &[Value]) -> anyhow::Result<()> {
    // Collect all unique keys as columns
    let mut columns: Vec<String> = Vec::new();
    for item in arr {
        if let Value::Object(obj) = item {
            for key in obj.keys() {
                if !columns.contains(key) {
                    columns.push(key.clone());
                }
            }
        }
    }

    if columns.is_empty() {
        return super::json::render(&Value::Array(arr.to_vec()));
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL_CONDENSED)
        .set_content_arrangement(ContentArrangement::Dynamic);

    table.set_header(&columns);

    for item in arr {
        if let Value::Object(obj) = item {
            let row: Vec<String> = columns
                .iter()
                .map(|col| format_cell(obj.get(col).unwrap_or(&Value::Null)))
                .collect();
            table.add_row(row);
        }
    }

    println!("{table}");
    Ok(())
}

/// Render a single object as a key-value table.
fn render_object(obj: &serde_json::Map<String, Value>) -> anyhow::Result<()> {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL_CONDENSED)
        .set_content_arrangement(ContentArrangement::Dynamic);

    table.set_header(vec!["Key", "Value"]);

    for (key, value) in obj {
        table.add_row(vec![key.clone(), format_cell(value)]);
    }

    println!("{table}");
    Ok(())
}

/// Format a JSON value for table cell display.
fn format_cell(value: &Value) -> String {
    match value {
        Value::Null => "".to_string(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Array(a) => {
            if a.len() <= 5 {
                let items: Vec<String> = a.iter().map(|v| format_cell(v)).collect();
                items.join(", ")
            } else {
                format!("[{} items]", a.len())
            }
        }
        Value::Object(_) => {
            // Compact JSON for nested objects
            serde_json::to_string(value).unwrap_or_else(|_| "{...}".to_string())
        }
    }
}
