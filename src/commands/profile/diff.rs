use crate::config::Environment;
use crate::parser::profile_parser;
use serde_json::{json, Value};

/// Compare two profiles: which operators got slower, where rows exploded, where skew appeared.
pub async fn run(slow_qid: &str, fast_qid: &str, env: &Environment) -> anyhow::Result<Value> {
    // Fetch both profiles — diff inherits fan-out across FEs automatically.
    let slow_fetched = super::fetch::fetch_profile_text(slow_qid, env)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let fast_fetched = super::fetch::fetch_profile_text(fast_qid, env)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let slow_profile = profile_parser::parse(&slow_fetched.text);
    let fast_profile = profile_parser::parse(&fast_fetched.text);

    let slow_ops = profile_parser::flatten_operators(&slow_profile);
    let fast_ops = profile_parser::flatten_operators(&fast_profile);

    let slow_total_ms = slow_profile.summary.total_time_ms.unwrap_or(0.0);
    let fast_total_ms = fast_profile.summary.total_time_ms.unwrap_or(0.0);
    let time_ratio = if fast_total_ms > 0.0 {
        slow_total_ms / fast_total_ms
    } else {
        0.0
    };

    // Match operators by name+fragment for comparison
    let mut operator_diffs = Vec::new();

    for slow_op in &slow_ops {
        // Find matching operator in fast run (by name and fragment)
        let fast_op = fast_ops.iter().find(|f| {
            f.name == slow_op.name && f.frag == slow_op.frag && f.pipeline == slow_op.pipeline
        });

        if let Some(fast_op) = fast_op {
            let time_delta_ms = slow_op.exec_time_avg_ms - fast_op.exec_time_avg_ms;
            let time_ratio_op = if fast_op.exec_time_avg_ms > 0.0 {
                slow_op.exec_time_avg_ms / fast_op.exec_time_avg_ms
            } else {
                0.0
            };

            let rows_delta = match (slow_op.input_rows, fast_op.input_rows) {
                (Some(s), Some(f)) if f > 0.0 => Some(round2(s / f)),
                _ => None,
            };

            let skew_delta = match (slow_op.skew_ratio, fast_op.skew_ratio) {
                (Some(s), Some(f)) => Some(round2(s - f)),
                _ => None,
            };

            // Only include operators with meaningful differences
            let significant = time_delta_ms.abs() > 1.0
                || rows_delta.map(|r| (r - 1.0).abs() > 0.1).unwrap_or(false)
                || skew_delta.map(|s| s.abs() > 0.5).unwrap_or(false);

            if significant {
                operator_diffs.push(json!({
                    "name": slow_op.name,
                    "frag": slow_op.frag,
                    "slow_ms": slow_op.exec_time_avg_ms,
                    "fast_ms": fast_op.exec_time_avg_ms,
                    "time_delta_ms": round2(time_delta_ms),
                    "time_ratio": round2(time_ratio_op),
                    "rows_ratio": rows_delta,
                    "skew_delta": skew_delta,
                    "slow_selectivity": slow_op.selectivity,
                    "fast_selectivity": fast_op.selectivity,
                    "slow_spilled": slow_op.spilled,
                    "fast_spilled": fast_op.spilled,
                }));
            }
        } else if slow_op.exec_time_avg_ms > 1.0 {
            // Operator exists only in slow run
            operator_diffs.push(json!({
                "name": slow_op.name,
                "frag": slow_op.frag,
                "slow_ms": slow_op.exec_time_avg_ms,
                "fast_ms": null,
                "time_delta_ms": slow_op.exec_time_avg_ms,
                "note": "only in slow run",
            }));
        }
    }

    // Sort by time_delta descending (biggest regressions first)
    operator_diffs.sort_by(|a, b| {
        let da = a
            .get("time_delta_ms")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let db = b
            .get("time_delta_ms")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(json!({
        "slow": {
            "query_id": slow_qid,
            "total_time_ms": slow_total_ms,
            "total_time": slow_profile.summary.total_time,
            "operator_count": slow_ops.len(),
        },
        "fast": {
            "query_id": fast_qid,
            "total_time_ms": fast_total_ms,
            "total_time": fast_profile.summary.total_time,
            "operator_count": fast_ops.len(),
        },
        "time_ratio": round2(time_ratio),
        "operator_diffs": operator_diffs,
    }))
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}
