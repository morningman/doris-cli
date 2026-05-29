use once_cell::sync::Lazy;
use regex::Regex;

/// Parsed operator info from the operator header line.
#[derive(Debug, Clone)]
pub struct OperatorInfo {
    pub full_name: String,
    pub operator_type: String,
    pub id: i32,
    pub nereids_id: Option<i32>,
    pub table_name: Option<String>,
    pub dest_id: Option<i32>,
    pub is_sink: bool,
}

// Match any operator line containing (id=N):
// Captures: 1=everything before (id=, 2=id, 3=nereids_id, 4=dst_id
static OPERATOR_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(.+?)\(?id=(-?\d+)(?:\s*,?\s*nereids_id=(\d+))?(?:\s*,?\s*dst_id=(\d+))?\)\s*:")
        .unwrap()
});

// Extract table_name from Doris 4.0 operator header:
// "OLAP_SCAN_OPERATOR(nereids_id=556. table_name=orders(orders))(id=2):"
static TABLE_NAME_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"table_name=(\w+)").unwrap());

impl OperatorInfo {
    /// Parse an operator header line.
    pub fn parse(line: &str) -> Option<Self> {
        let caps = OPERATOR_RE.captures(line)?;

        let full_name = caps[1].trim().to_string();
        let id: i32 = caps[2].parse().ok()?;
        let nereids_id: Option<i32> = caps.get(3).and_then(|m| m.as_str().parse().ok());
        let dest_id: Option<i32> = caps.get(4).and_then(|m| m.as_str().parse().ok());

        // Extract the base operator type (without parenthesized qualifiers)
        let operator_type = full_name
            .split('(')
            .next()
            .unwrap_or(&full_name)
            .trim()
            .to_string();

        let is_sink = operator_type.contains("SINK");

        // Extract table_name from Doris 4.0 header format
        let table_name = TABLE_NAME_RE.captures(line).map(|c| c[1].to_string());

        Some(OperatorInfo {
            full_name,
            operator_type,
            id,
            nereids_id,
            table_name,
            dest_id,
            is_sink,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple() {
        let info = OperatorInfo::parse("RESULT_SINK_OPERATOR (id=0):").unwrap();
        assert_eq!(info.operator_type, "RESULT_SINK_OPERATOR");
        assert_eq!(info.id, 0);
        assert!(info.is_sink);
    }

    #[test]
    fn test_parse_with_nereids() {
        let info = OperatorInfo::parse("SORT_OPERATOR (id=13 , nereids_id=1617):").unwrap();
        assert_eq!(info.id, 13);
        assert_eq!(info.nereids_id, Some(1617));
    }

    #[test]
    fn test_parse_with_local_exchange() {
        let info =
            OperatorInfo::parse("LOCAL_EXCHANGE_OPERATOR (LOCAL_MERGE_SORT) (id=-4):").unwrap();
        assert_eq!(info.id, -4);
        assert!(info.full_name.contains("LOCAL_MERGE_SORT"));
    }

    #[test]
    fn test_parse_dst_id() {
        let info = OperatorInfo::parse("DATA_STREAM_SINK_OPERATOR (id=14,dst_id=14):").unwrap();
        assert_eq!(info.dest_id, Some(14));
    }

    #[test]
    fn test_parse_doris4_scan_with_table() {
        let info = OperatorInfo::parse(
            "OLAP_SCAN_OPERATOR(nereids_id=556. table_name=orders(orders))(id=2):",
        )
        .unwrap();
        assert_eq!(info.id, 2);
        assert_eq!(info.table_name, Some("orders".to_string()));
        assert!(info.operator_type.contains("OLAP_SCAN"));
    }
}
