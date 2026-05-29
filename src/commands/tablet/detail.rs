use super::overview::parse_table_name;
use crate::config::Environment;
use crate::connection::MysqlConnection;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// --detail: distribution + per-tablet + backend mapping in ONE call.
pub async fn run(
    table: &str,
    partition_filter: Option<&str>,
    env: &Environment,
) -> anyhow::Result<Value> {
    let (db, tbl) = parse_table_name(table);
    let mut conn = MysqlConnection::connect(env).await?;

    if let Some(db) = &db {
        conn.exec(&format!("USE `{db}`")).await?;
    }

    // Get all tablets (optionally filtered by partition)
    let tablet_sql = if let Some(part) = partition_filter {
        format!("SHOW TABLETS FROM `{tbl}` PARTITION(`{part}`)")
    } else {
        format!("SHOW TABLETS FROM `{tbl}`")
    };
    let tablet_result = conn.query(&tablet_sql).await?;

    // Get partitions for context
    let part_result = conn.query(&format!("SHOW PARTITIONS FROM `{tbl}`")).await?;

    // Build partition name lookup: PartitionId → PartitionName
    let mut partition_names: BTreeMap<String, String> = BTreeMap::new();
    let mut partition_buckets: BTreeMap<String, u64> = BTreeMap::new();
    for row in &part_result.rows {
        let pid = row
            .get("PartitionId")
            .and_then(|v| v.as_str().or_else(|| v.as_u64().map(|_| "")).or(Some("")))
            .unwrap_or("")
            .to_string();
        let pname = row
            .get("PartitionName")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let buckets = row
            .get("Buckets")
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(0);
        partition_names.insert(pid, pname.clone());
        partition_buckets.insert(pname, buckets);
    }

    // Build per-tablet data and aggregate by partition + backend
    let mut tablets = Vec::new();
    let mut partition_stats: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut backend_stats: BTreeMap<u64, (u64, f64)> = BTreeMap::new(); // backend_id → (count, total_mb)

    // Determine partition name: try matching from SHOW PARTITIONS, else use filter or "default"
    let default_partition = partition_filter
        .map(|s| s.to_string())
        .or_else(|| {
            part_result.rows.first().and_then(|r| {
                r.get("PartitionName")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
        })
        .unwrap_or_else(|| tbl.to_string());

    for row in &tablet_result.rows {
        let tablet_id = row.get("TabletId").and_then(|v| v.as_u64()).unwrap_or(0);
        let backend_id = row.get("BackendId").and_then(|v| v.as_u64()).unwrap_or(0);
        let size_bytes = row
            .get("LocalDataSize")
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(0);
        let size_mb = size_bytes as f64 / (1024.0 * 1024.0);
        let row_count = row
            .get("RowCount")
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(0);
        let version = row
            .get("Version")
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(0);
        let state = row
            .get("State")
            .and_then(|v| v.as_str())
            .unwrap_or("UNKNOWN");

        // For single-partition tables, use the default partition name
        let partition = default_partition.clone();

        partition_stats
            .entry(partition.clone())
            .or_default()
            .push(size_mb);

        let entry = backend_stats.entry(backend_id).or_insert((0, 0.0));
        entry.0 += 1;
        entry.1 += size_mb;

        tablets.push(json!({
            "tablet_id": tablet_id,
            "partition": partition,
            "backend_id": backend_id,
            "size_mb": round1(size_mb),
            "row_count": row_count,
            "version": version,
            "state": state,
        }));
    }

    // Aggregate partition stats
    let partitions: Vec<Value> = partition_stats
        .iter()
        .map(|(name, sizes)| {
            let count = sizes.len() as f64;
            let sum: f64 = sizes.iter().sum();
            let avg = sum / count;
            let min = sizes.iter().cloned().fold(f64::INFINITY, f64::min);
            let max = sizes.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let skew = if avg > 0.0 { max / avg } else { 1.0 };

            json!({
                "name": name,
                "tablets": sizes.len(),
                "total_mb": round1(sum),
                "avg_mb": round1(avg),
                "min_mb": round1(min),
                "max_mb": round1(max),
                "skew_ratio": round1(skew),
            })
        })
        .collect();

    // Backend distribution
    let backends: Vec<Value> = backend_stats
        .iter()
        .map(|(id, (count, total_mb))| {
            json!({
                "backend_id": id,
                "tablet_count": count,
                "total_mb": round1(*total_mb),
            })
        })
        .collect();

    Ok(json!({
        "partitions": partitions,
        "tablets": tablets,
        "backends": backends,
    }))
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}
