use serde_json::Value;

/// Render a JSON value as CSV to stdout.
pub fn render(value: &Value) -> anyhow::Result<()> {
    match value {
        Value::Array(arr) if !arr.is_empty() => render_array(arr),
        _ => {
            // Fallback to JSON for non-tabular data
            super::json::render(value)
        }
    }
}

fn render_array(arr: &[Value]) -> anyhow::Result<()> {
    // Collect columns
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

    // Header
    println!("{}", columns.join(","));

    // Rows
    for item in arr {
        if let Value::Object(obj) = item {
            let row: Vec<String> = columns
                .iter()
                .map(|col| csv_escape(obj.get(col).unwrap_or(&Value::Null)))
                .collect();
            println!("{}", row.join(","));
        }
    }

    Ok(())
}

fn csv_escape(value: &Value) -> String {
    let s = match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => serde_json::to_string(other).unwrap_or_default(),
    };

    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}
