use crate::config::Environment;
use crate::connection::MysqlConnection;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};

static MODEL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(DUPLICATE|UNIQUE|AGGREGATE)\s+KEY").unwrap());
static DIST_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)DISTRIBUTED\s+BY\s+(HASH|RANDOM)\s*\(([^)]*)\)\s*BUCKETS\s*(\d+)").unwrap()
});
static AUTO_BUCKET_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)DISTRIBUTED\s+BY\s+(HASH|RANDOM)\s*\(([^)]*)\)\s*BUCKETS\s*AUTO").unwrap()
});
static KEY_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(DUPLICATE|UNIQUE|AGGREGATE)\s+KEY\s*\(([^)]+)\)").unwrap());
static REPL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#""replication_num"\s*=\s*"(\d+)""#).unwrap());

/// Default: overview + keys (SHOW COLUMN STATS) + health (SHOW DATA SKEW) in ONE call.
pub async fn run(table: &str, env: &Environment) -> anyhow::Result<Value> {
    let (db, tbl) = parse_table_name(table);
    let mut conn = MysqlConnection::connect(env).await?;

    if let Some(db) = &db {
        conn.exec(&format!("USE `{db}`")).await?;
    }

    // ── 1. DDL parsing ──
    let ddl_result = conn.query(&format!("SHOW CREATE TABLE `{tbl}`")).await?;
    let ddl = ddl_result
        .rows
        .first()
        .and_then(|r| r.get("Create Table"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let model = MODEL_RE
        .captures(&ddl)
        .map(|c| c[1].to_uppercase())
        .unwrap_or_else(|| "UNKNOWN".to_string());

    let sort_key: Vec<String> = KEY_RE
        .captures(&ddl)
        .map(|c| {
            c[2].split(',')
                .map(|k| k.trim().trim_matches('`').to_string())
                .collect()
        })
        .unwrap_or_default();

    let (bucket_type, bucket_key, bucket_count) = if let Some(caps) = DIST_RE.captures(&ddl) {
        let btype = caps[1].to_uppercase();
        let keys: Vec<String> = caps[2]
            .split(',')
            .map(|k| k.trim().trim_matches('`').to_string())
            .collect();
        let count: u32 = caps[3].parse().unwrap_or(0);
        (btype, keys, count)
    } else if let Some(caps) = AUTO_BUCKET_RE.captures(&ddl) {
        let btype = caps[1].to_uppercase();
        let keys: Vec<String> = caps[2]
            .split(',')
            .map(|k| k.trim().trim_matches('`').to_string())
            .collect();
        (btype, keys, 0u32)
    } else {
        ("UNKNOWN".to_string(), vec![], 0u32)
    };

    let replication_num: u32 = REPL_RE
        .captures(&ddl)
        .and_then(|c| c[1].parse().ok())
        .unwrap_or(1);

    // ── 2. Partition summary ──
    let part_result = conn.query(&format!("SHOW PARTITIONS FROM `{tbl}`")).await?;

    let mut total_size_bytes: f64 = 0.0;
    let mut total_rows: u64 = 0;
    let mut partition_count: u32 = 0;
    let mut empty_partitions: u32 = 0;
    let mut max_part_name = String::new();
    let mut max_part_size: f64 = 0.0;
    let mut min_part_name = String::new();
    let mut min_part_size: f64 = f64::MAX;
    let mut actual_bucket_count = bucket_count;

    for row in &part_result.rows {
        partition_count += 1;
        let data_size = row.get("DataSize").and_then(|v| v.as_str()).unwrap_or("0");
        let size_bytes = parse_size_string(data_size);
        let row_count = row
            .get("RowCount")
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(0);

        if partition_count == 1 {
            if let Some(buckets) = row.get("Buckets").and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            }) {
                actual_bucket_count = buckets as u32;
            }
        }

        total_size_bytes += size_bytes;
        total_rows += row_count;

        if row_count == 0 && size_bytes < 1.0 {
            empty_partitions += 1;
        }

        let pname = row
            .get("PartitionName")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if size_bytes > max_part_size {
            max_part_size = size_bytes;
            max_part_name = pname.to_string();
        }
        if size_bytes < min_part_size {
            min_part_size = size_bytes;
            min_part_name = pname.to_string();
        }
    }

    if partition_count == 0 {
        min_part_size = 0.0;
    }

    // ── 3. Column stats (SHOW COLUMN STATS — fast, no scan) ──
    let columns = get_column_stats(&mut conn, &tbl).await;

    // ── 4. Health from SHOW DATA SKEW ──
    let health = get_health_summary(&mut conn, &tbl).await;

    let full_table = if let Some(db) = &db {
        format!("{db}.{tbl}")
    } else {
        tbl.to_string()
    };

    Ok(json!({
        "table": full_table,
        "model": model,
        "bucket_type": bucket_type,
        "bucket_key": bucket_key,
        "bucket_count": actual_bucket_count,
        "replication_num": replication_num,
        "sort_key": sort_key,
        "total_size_gb": round2(total_size_bytes / (1024.0 * 1024.0 * 1024.0)),
        "total_rows": total_rows,
        "partitions": partition_count,
        "empty_partitions": empty_partitions,
        "max_partition": {"name": max_part_name, "size_gb": round2(max_part_size / (1024.0 * 1024.0 * 1024.0))},
        "min_partition": {"name": min_part_name, "size_gb": round2(min_part_size / (1024.0 * 1024.0 * 1024.0))},
        "columns": columns,
        "health": health,
    }))
}

/// Get column stats from SHOW COLUMN STATS (fast path).
async fn get_column_stats(conn: &mut MysqlConnection, tbl: &str) -> Vec<Value> {
    let result = match conn.query(&format!("SHOW COLUMN STATS `{tbl}`")).await {
        Ok(r) if !r.rows.is_empty() => r,
        _ => return vec![],
    };

    result
        .rows
        .iter()
        .map(|row| {
            let ndv = row
                .get("ndv")
                .and_then(|v| {
                    v.as_f64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                })
                .unwrap_or(0.0);
            let count = row
                .get("count")
                .and_then(|v| {
                    v.as_f64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                })
                .unwrap_or(1.0)
                .max(1.0);
            let null_count = row
                .get("num_null")
                .and_then(|v| {
                    v.as_f64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                })
                .unwrap_or(0.0);

            json!({
                "name": row.get("column_name").and_then(|v| v.as_str()).unwrap_or(""),
                "ndv": ndv as u64,
                "null_pct": round1((null_count / count) * 100.0),
                "min": row.get("min").and_then(|v| v.as_str()),
                "max": row.get("max").and_then(|v| v.as_str()),
            })
        })
        .collect()
}

/// Get health summary from SHOW DATA SKEW.
async fn get_health_summary(conn: &mut MysqlConnection, tbl: &str) -> Value {
    let result = match conn.query(&format!("SHOW DATA SKEW FROM `{tbl}`")).await {
        Ok(r) if !r.rows.is_empty() => r,
        _ => return json!(null),
    };

    let mut sizes: Vec<f64> = Vec::new();
    for row in &result.rows {
        let size = row
            .get("AvgDataSize")
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(0) as f64;
        sizes.push(size / (1024.0 * 1024.0));
    }

    if sizes.is_empty() {
        return json!(null);
    }

    let count = sizes.len() as f64;
    let avg = sizes.iter().sum::<f64>() / count;
    let max = sizes.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let min = sizes.iter().cloned().fold(f64::INFINITY, f64::min);
    let skew = if avg > 0.0 { max / avg } else { 1.0 };

    json!({
        "tablets": sizes.len(),
        "avg_tablet_mb": round1(avg),
        "min_tablet_mb": round1(min),
        "max_tablet_mb": round1(max),
        "tablet_skew": round1(skew),
    })
}

/// Parse a Doris size string like "49.618 MB" to bytes.
pub fn parse_size_string(s: &str) -> f64 {
    let s = s.trim();
    if s.is_empty() || s == "0" || s == "0.00" {
        return 0.0;
    }
    static SIZE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"([\d.]+)\s*(TB|GB|MB|KB|B)?").unwrap());
    if let Some(caps) = SIZE_RE.captures(s) {
        let num: f64 = caps[1].parse().unwrap_or(0.0);
        let unit = caps.get(2).map(|m| m.as_str()).unwrap_or("B");
        match unit {
            "TB" => num * 1024.0 * 1024.0 * 1024.0 * 1024.0,
            "GB" => num * 1024.0 * 1024.0 * 1024.0,
            "MB" => num * 1024.0 * 1024.0,
            "KB" => num * 1024.0,
            _ => num,
        }
    } else {
        0.0
    }
}

/// Parse "db.table" or just "table".
pub fn parse_table_name(table: &str) -> (Option<String>, String) {
    if let Some(dot) = table.find('.') {
        (Some(table[..dot].to_string()), table[dot + 1..].to_string())
    } else {
        (None, table.to_string())
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}
