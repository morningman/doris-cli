use crate::config::Environment;
use crate::connection::MysqlConnection;
use serde_json::{json, Value};

/// Show execution history for a query pattern from __internal_schema.audit_log.
/// Provides trend analysis: p50/p99 latency, degradation detection.
pub async fn run(
    sql_pattern: &str,
    days: u32,
    limit: usize,
    env: &Environment,
) -> anyhow::Result<Value> {
    let mut conn = MysqlConnection::connect(env).await?;

    // Escape single quotes in pattern for SQL safety
    let safe_pattern = sql_pattern.replace('\'', "''");

    // Query audit_log for matching queries
    let sql = format!(
        "SELECT \
            query_id, \
            stmt AS sql_text, \
            query_time AS query_time_ms, \
            scan_bytes, \
            scan_rows, \
            return_rows, \
            shuffle_send_bytes, \
            peak_memory_bytes, \
            cpu_time_ms, \
            `time` AS exec_time, \
            state \
         FROM `__internal_schema`.`audit_log` \
         WHERE stmt LIKE '%{safe_pattern}%' \
           AND `time` >= DATE_SUB(NOW(), INTERVAL {days} DAY) \
           AND state != 'ERR' \
         ORDER BY `time` DESC \
         LIMIT {limit}"
    );

    let result = match conn.query(&sql).await {
        Ok(r) => r,
        Err(e) => {
            // audit_log may not be accessible
            anyhow::bail!(
                "Cannot query audit_log: {e}\n\
                 The __internal_schema.audit_log table may not be enabled on this cluster."
            );
        }
    };

    if result.rows.is_empty() {
        return Ok(json!({
            "pattern": sql_pattern,
            "days": days,
            "executions": 0,
            "entries": [],
            "note": "No matching queries found in audit_log"
        }));
    }

    // Compute aggregate statistics
    let mut query_times: Vec<f64> = Vec::new();
    let mut scan_bytes_total: f64 = 0.0;
    let mut peak_mem_total: f64 = 0.0;

    let entries: Vec<Value> = result
        .rows
        .iter()
        .map(|row| {
            let qt = row
                .get("query_time_ms")
                .and_then(|v| {
                    v.as_f64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                })
                .unwrap_or(0.0);
            query_times.push(qt);

            let sb = row
                .get("scan_bytes")
                .and_then(|v| {
                    v.as_f64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                })
                .unwrap_or(0.0);
            scan_bytes_total += sb;

            let pm = row
                .get("peak_memory_bytes")
                .and_then(|v| {
                    v.as_f64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                })
                .unwrap_or(0.0);
            peak_mem_total += pm;

            json!({
                "query_id": row.get("query_id").and_then(|v| v.as_str()).unwrap_or(""),
                "exec_time": row.get("exec_time").and_then(|v| v.as_str()).unwrap_or(""),
                "query_time_ms": qt,
                "scan_bytes": sb as u64,
                "scan_rows": row.get("scan_rows").and_then(|v| v.as_u64()),
                "return_rows": row.get("return_rows").and_then(|v| v.as_u64()),
                "peak_memory_bytes": pm as u64,
                "state": row.get("state").and_then(|v| v.as_str()).unwrap_or(""),
            })
        })
        .collect();

    // Compute percentiles
    query_times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let count = query_times.len();
    let p50 = if count > 0 {
        query_times[count / 2]
    } else {
        0.0
    };
    let p99 = if count > 0 {
        query_times[(count as f64 * 0.99) as usize].min(*query_times.last().unwrap_or(&0.0))
    } else {
        0.0
    };
    let avg = if count > 0 {
        query_times.iter().sum::<f64>() / count as f64
    } else {
        0.0
    };
    let min = query_times.first().copied().unwrap_or(0.0);
    let max = query_times.last().copied().unwrap_or(0.0);

    Ok(json!({
        "pattern": sql_pattern,
        "days": days,
        "executions": count,
        "stats": {
            "query_time_ms": {
                "avg": round1(avg),
                "min": round1(min),
                "max": round1(max),
                "p50": round1(p50),
                "p99": round1(p99),
            },
            "avg_scan_bytes": round1(scan_bytes_total / count as f64),
            "avg_peak_memory_bytes": round1(peak_mem_total / count as f64),
        },
        "entries": entries,
    }))
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}
