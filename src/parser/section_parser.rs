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
        // Top-level profile section headers. Doris spells these differently across
        // versions: with or without internal spaces ("Changed Session Variables" vs
        // "ChangedSessionVariables", "Physical Plan" vs "PhysicalPlan"), and some carry a
        // parenthesized suffix ("DetailProfile(<query_id>):"). Match every spelling and
        // normalize to a canonical key (see canonical_section). DetailProfile and Appendix
        // are matched purely as boundaries so their bodies can't bleed into the preceding
        // MergedProfile section — which would fabricate empty fragments.
        Regex::new(
            r"(?m)^\s*(Summary|Execution ?Summary|Changed ?Session ?Variables|Physical ?Plan|MergedProfile|DetailProfile|Appendix)\b.*:\s*$",
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
            current_name = Some(canonical_section(&caps[1]));
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

/// Normalize a section header to a canonical lookup key, so spelling variants
/// across Doris versions (with/without internal spaces) resolve to the one key
/// the parsers query for.
fn canonical_section(raw_name: &str) -> String {
    let compact: String = raw_name.split_whitespace().collect();
    match compact.as_str() {
        "ExecutionSummary" => "Execution Summary".to_string(),
        "ChangedSessionVariables" => "Changed Session Variables".to_string(),
        "PhysicalPlan" => "Physical Plan".to_string(),
        _ => compact,
    }
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

    #[test]
    fn test_split_sections_spellings_and_boundaries() {
        // No-space spellings + a parenthesized DetailProfile header that must act as a
        // boundary so its "Fragment 0:" cannot leak into MergedProfile.
        let text = "\
MergedProfile:
      Fragments:
        Fragment 0:
          Pipeline 0(instance_num=1):
DetailProfile(abc-123):
  Fragments:
    Fragment 0:
      Pipeline 0(host=h):
ChangedSessionVariables:
[ { \"VarName\": \"enable_profile\" } ]
PhysicalPlan:
PhysicalResultSink[1]
";
        let sections = split_sections(text);

        // Canonical keys exist for the no-space spellings.
        assert!(sections.contains_key("Changed Session Variables"));
        assert!(sections.contains_key("Physical Plan"));

        // DetailProfile is its own section, so MergedProfile stops before it and does
        // not contain the DetailProfile body.
        let merged = sections.get("MergedProfile").expect("MergedProfile present");
        assert!(merged.contains("Pipeline 0(instance_num=1)"));
        assert!(
            !merged.contains("DetailProfile") && !merged.contains("host=h"),
            "DetailProfile body must not bleed into MergedProfile, got: {merged}"
        );

        // Physical Plan body is captured, not swallowed by the previous section.
        assert!(sections
            .get("Physical Plan")
            .map(|p| p.contains("PhysicalResultSink"))
            .unwrap_or(false));
    }
}
