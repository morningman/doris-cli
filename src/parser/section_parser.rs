use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

/// Collapse double-spaced text from Doris 3.0 web export.
/// "Profile  ID" → "Profile ID", but preserve indentation.
pub fn normalize_text(input: &str) -> String {
    let mut result = String::with_capacity(input.len());

    for line in input.lines() {
        // Preserve leading whitespace
        let trimmed = line.trim_end();
        let leading = &trimmed[..trimmed.len() - trimmed.trim_start().len()];
        let content = trimmed.trim_start();

        // Collapse runs of 2+ spaces within the content (not leading indent)
        static MULTI_SPACE: Lazy<Regex> = Lazy::new(|| Regex::new(r"  +").unwrap());
        let normalized = MULTI_SPACE.replace_all(content, " ");

        result.push_str(leading);
        result.push_str(&normalized);
        result.push('\n');
    }

    // Also remove \r
    result.replace('\r', "")
}

/// Split a normalized profile text into named sections.
pub fn split_sections(text: &str) -> HashMap<String, String> {
    static SECTION_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?m)^\s*(Summary|Execution Summary|Changed Session Variables|Physical Plan|MergedProfile)\s*:?\s*$",
        )
        .unwrap()
    });

    let mut sections = HashMap::new();
    let mut current_name: Option<String> = None;
    let mut current_content = String::new();

    for line in text.lines() {
        if let Some(caps) = SECTION_RE.captures(line) {
            // Save previous section
            if let Some(name) = current_name.take() {
                sections.insert(name, current_content.trim().to_string());
                current_content.clear();
            }
            current_name = Some(caps[1].to_string());
        } else if current_name.is_some() {
            current_content.push_str(line);
            current_content.push('\n');
        }
    }

    // Save last section
    if let Some(name) = current_name {
        sections.insert(name, current_content.trim().to_string());
    }

    sections
}

/// Parse key-value pairs from Summary or Execution Summary sections.
/// Lines look like: "      - Key: Value"
pub fn parse_kv_section(text: &str) -> HashMap<String, String> {
    static KV_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*-\s+(.+?):\s+(.+)$").unwrap());

    let mut map = HashMap::new();
    for line in text.lines() {
        if let Some(caps) = KV_RE.captures(line) {
            let key = caps[1].trim().to_string();
            let value = caps[2].trim().to_string();
            map.insert(key, value);
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_double_spaces() {
        let result = normalize_text("      -  Profile  ID:  abc123");
        // Leading indent is preserved, double spaces in content collapsed
        assert!(result.contains("- Profile ID: abc123"));
    }

    #[test]
    fn test_parse_kv() {
        let text = "      - Total: 32sec281ms\n      - Task State: OK\n";
        let kv = parse_kv_section(text);
        assert_eq!(kv.get("Total"), Some(&"32sec281ms".to_string()));
        assert_eq!(kv.get("Task State"), Some(&"OK".to_string()));
    }
}
