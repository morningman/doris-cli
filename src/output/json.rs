use serde_json::Value;

/// Render a JSON value as pretty-printed JSON to stdout.
pub fn render(value: &Value) -> anyhow::Result<()> {
    let output = serde_json::to_string_pretty(value)?;
    println!("{output}");
    Ok(())
}
