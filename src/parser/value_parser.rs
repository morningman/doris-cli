use once_cell::sync::Lazy;
use regex::Regex;

static TIME_COMPONENT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(\d+(?:\.\d+)?)\s*(ms|us|μs|ns|h|m(?:in)?|s(?:ec)?)").unwrap());

static BYTES_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^([\d,.]+)\s*(TB|GB|MB|KB|B)?").unwrap());

/// Parse a Doris duration string to milliseconds.
/// Examples: "32sec281ms", "150.364us", "0ns", "1h2m3sec", "442.308ms"
pub fn parse_duration_ms(input: &str) -> Option<f64> {
    let input = input.trim();

    if input == "0" || input == "N/A" {
        return Some(0.0);
    }

    let mut total_ns: f64 = 0.0;
    let mut found = false;

    for cap in TIME_COMPONENT_RE.captures_iter(input) {
        found = true;
        let num: f64 = cap[1].parse().ok()?;
        let unit = &cap[2];

        let ns = match unit {
            "h" => num * 3600.0 * 1e9,
            "m" | "min" => num * 60.0 * 1e9,
            "s" | "sec" => num * 1e9,
            "ms" => num * 1e6,
            "us" | "μs" => num * 1e3,
            "ns" => num,
            _ => 0.0,
        };
        total_ns += ns;
    }

    if found {
        Some(total_ns / 1e6) // Convert ns to ms
    } else {
        None
    }
}

/// Parse a bytes string like "57.94 KB", "8.62 MB", "0.00 " to bytes.
pub fn parse_bytes(input: &str) -> Option<f64> {
    let input = input.trim();
    if input.is_empty() || input == "0" || input == "0.00" {
        return Some(0.0);
    }

    if let Some(caps) = BYTES_RE.captures(input) {
        let num_str = caps[1].replace(',', "");
        let num: f64 = num_str.parse().ok()?;
        let unit = caps.get(2).map(|m| m.as_str()).unwrap_or("B");
        let bytes = match unit {
            "TB" => num * 1024.0 * 1024.0 * 1024.0 * 1024.0,
            "GB" => num * 1024.0 * 1024.0 * 1024.0,
            "MB" => num * 1024.0 * 1024.0,
            "KB" => num * 1024.0,
            _ => num,
        };
        Some(bytes)
    } else {
        None
    }
}

/// Parse a number string, handling commas: "3,301,515,299" → 3301515299.0
pub fn parse_number(input: &str) -> Option<f64> {
    let cleaned = input.trim().replace(',', "");
    cleaned.parse::<f64>().ok()
}

/// Parse a counter value — could be duration, bytes, or plain number.
/// Doris profile format: "3.301515299B (3301515299)" — parenthetical is the exact value.
/// Returns the numeric value (ms for time, bytes for size, raw for counts).
pub fn parse_counter_value(input: &str) -> f64 {
    let input = input.trim();

    // First: extract parenthetical value if present (highest precision)
    // e.g., "3.301515299B (3301515299)" → use 3301515299
    if let Some(start) = input.find('(') {
        if let Some(end) = input.find(')') {
            let paren_val = input[start + 1..end].trim();
            if let Ok(n) = paren_val.parse::<f64>() {
                return n;
            }
        }
    }

    // Try duration first
    if let Some(ms) = parse_duration_ms(input) {
        return ms;
    }

    // Try bytes
    if let Some(bytes) = parse_bytes(input) {
        return bytes;
    }

    // Try plain number
    if let Some(n) = parse_number(input) {
        return n;
    }

    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration() {
        assert!((parse_duration_ms("32sec281ms").unwrap() - 32281.0).abs() < 0.1);
        assert!((parse_duration_ms("150.364us").unwrap() - 0.150364).abs() < 0.001);
        assert!((parse_duration_ms("0ns").unwrap() - 0.0).abs() < 0.001);
        assert!((parse_duration_ms("442.308ms").unwrap() - 442.308).abs() < 0.001);
        assert!((parse_duration_ms("9sec169ms").unwrap() - 9169.0).abs() < 0.1);
        assert!((parse_duration_ms("1ms").unwrap() - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_parse_bytes() {
        assert!((parse_bytes("57.94 KB").unwrap() - 59330.56).abs() < 1.0);
        let mb_val = parse_bytes("8.62 MB").unwrap();
        assert!(
            mb_val > 9_000_000.0 && mb_val < 10_000_000.0,
            "Got {mb_val}"
        );
        assert!((parse_bytes("0.00 ").unwrap() - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_parse_number() {
        assert!((parse_number("3,301,515,299").unwrap() - 3301515299.0).abs() < 1.0);
        assert!((parse_number("24").unwrap() - 24.0).abs() < 0.001);
    }

    #[test]
    fn test_parse_counter_value_parenthetical() {
        // Doris format: "3.301515299B (3301515299)" — should extract 3301515299
        assert!((parse_counter_value("3.301515299B (3301515299)") - 3301515299.0).abs() < 1.0);
        assert!((parse_counter_value("22.927189M (22927189)") - 22927189.0).abs() < 1.0);
        // Without parens — should still work via duration/bytes/number parsing
        assert!((parse_counter_value("150.364us") - 0.150364).abs() < 0.001);
        assert!((parse_counter_value("57.94 KB") - 59330.56).abs() < 1.0);
        assert!((parse_counter_value("24") - 24.0).abs() < 0.001);
    }
}
