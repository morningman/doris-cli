use crate::config::Environment;
use crate::connection::MysqlConnection;
use crate::parser::{profile_parser, section_parser};
use serde_json::{json, Value};

/// --full: Complete diagnostic snapshot.
/// - profile: full parsed tree (fragments → pipelines → operators with all_counters)
/// - operators: flattened with diagnostic fields (selectivity, spill, shuffle, cache, wait)
/// - physical_plan: query plan text
/// - scanned_tables: DDL + column_stats + tablet health per table
/// - cluster: backends, workload groups, live session variables
pub async fn run(
    query_id: &str,
    profile_text: Option<&str>,
    env: &Environment,
) -> anyhow::Result<Value> {
    let (text, served_by, via, attempts) = if let Some(t) = profile_text {
        (t.to_string(), None, None, Vec::new())
    } else {
        let fetched = super::fetch::fetch_profile_text(query_id, env)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        (
            fetched.text,
            Some(fetched.served_by),
            Some(fetched.via),
            fetched.attempts,
        )
    };

    let profile = profile_parser::parse(&text);
    let flat_ops = profile_parser::flatten_operators(&profile);

    // Physical plan
    let normalized = section_parser::normalize_text(&text);
    let sections = section_parser::split_sections(&normalized);

    // Extract scanned table names
    let scanned_tables: Vec<String> = flat_ops
        .iter()
        .filter_map(|op| op.table.as_ref())
        .map(|t| t.split('(').next().unwrap_or(t).trim().to_string())
        .filter(|t| !t.is_empty())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    // Gather all table context + cluster info via connection
    let mut table_context: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut cluster = json!(null);

    if let Ok(mut conn) = MysqlConnection::connect(env).await {
        // USE database from table names
        let db_to_use = profile
            .summary
            .default_db
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                scanned_tables
                    .iter()
                    .find(|t| t.contains('.'))
                    .and_then(|t| t.split('.').next())
            });
        if let Some(db) = db_to_use {
            let _ = conn.exec(&format!("USE `{db}`")).await;
        }

        // ── Scanned table DDLs + tablet health ──
        for table in &scanned_tables {
            let mut entry = json!({});
            let tbl_name = table.split('.').last().unwrap_or(table);

            // DDL (full CREATE TABLE)
            let ddl_queries = if table.contains('.') {
                let parts: Vec<&str> = table.split('.').collect();
                vec![
                    format!("SHOW CREATE TABLE `{}`.`{}`", parts[0], parts[1]),
                    format!("SHOW CREATE TABLE `{}`", parts.last().unwrap_or(&"")),
                ]
            } else {
                vec![format!("SHOW CREATE TABLE `{table}`")]
            };

            for ddl_sql in &ddl_queries {
                if let Ok(ddl_result) = conn.query(ddl_sql).await {
                    if let Some(ddl) = ddl_result
                        .rows
                        .first()
                        .and_then(|r| r.get("Create Table"))
                        .and_then(|v| v.as_str())
                    {
                        entry["ddl"] = Value::String(ddl.to_string());
                        break;
                    }
                }
            }

            // Column stats
            if let Ok(stats) = conn.query(&format!("SHOW COLUMN STATS `{tbl_name}`")).await {
                let cols: Vec<Value> = stats
                    .rows
                    .iter()
                    .map(|r| {
                        json!({
                            "column": r.get("column_name").and_then(|v| v.as_str()),
                            "ndv": r.get("ndv").and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))),
                            "count": r.get("count").and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))),
                            "null_count": r.get("num_null").and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))),
                            "min": r.get("min").and_then(|v| v.as_str()),
                            "max": r.get("max").and_then(|v| v.as_str()),
                        })
                    })
                    .collect();
                if !cols.is_empty() {
                    entry["column_stats"] = Value::Array(cols);
                }
            }

            // Tablet health (DATA SKEW)
            if let Ok(skew) = conn
                .query(&format!("SHOW DATA SKEW FROM `{tbl_name}`"))
                .await
            {
                let mut sizes: Vec<f64> = Vec::new();
                let mut total_rows: u64 = 0;
                for row in &skew.rows {
                    let size = row
                        .get("AvgDataSize")
                        .and_then(|v| {
                            v.as_u64()
                                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                        })
                        .unwrap_or(0) as f64;
                    sizes.push(size);
                    total_rows += row
                        .get("AvgRowCount")
                        .and_then(|v| {
                            v.as_u64()
                                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                        })
                        .unwrap_or(0);
                }
                if !sizes.is_empty() {
                    let avg = sizes.iter().sum::<f64>() / sizes.len() as f64;
                    let max = sizes.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                    let total: f64 = sizes.iter().sum();
                    entry["tablets"] = json!(sizes.len());
                    entry["total_rows"] = json!(total_rows);
                    entry["total_size_gb"] = json!(round2(total / (1024.0 * 1024.0 * 1024.0)));
                    entry["tablet_skew"] = json!(round1(if avg > 0.0 { max / avg } else { 1.0 }));
                }
            }

            // Partitions
            if let Ok(parts) = conn
                .query(&format!("SHOW PARTITIONS FROM `{tbl_name}`"))
                .await
            {
                entry["partition_count"] = json!(parts.rows.len());
            }

            table_context.insert(table.clone(), entry);
        }

        // ── Cluster state ──
        let mut cluster_info = json!({});

        // Backends
        if let Ok(bes) = conn.query("SHOW BACKENDS").await {
            let backends: Vec<Value> = bes
                .rows
                .iter()
                .map(|row| {
                    json!({
                        "id": row.get("BackendId").or_else(|| row.get("backend_id")),
                        "host": row.get("Host").or_else(|| row.get("IP")).and_then(|v| v.as_str()),
                        "alive": row.get("Alive").and_then(|v| v.as_str()).map(|s| s == "true"),
                    })
                })
                .collect();
            cluster_info["backends"] = Value::Array(backends);
        }

        // Workload groups
        if let Ok(wgs) = conn.query("SHOW WORKLOAD GROUPS").await {
            let groups: Vec<Value> = wgs
                .rows
                .iter()
                .map(|row| {
                    json!({
                        "name": row.get("Name").and_then(|v| v.as_str()),
                        "max_cpu": row.get("max_cpu_percent").and_then(|v| v.as_str()),
                        "max_memory": row.get("max_memory_percent").and_then(|v| v.as_str()),
                        "running": row.get("running_query_num"),
                        "waiting": row.get("waiting_query_num"),
                    })
                })
                .collect();
            cluster_info["workload_groups"] = Value::Array(groups);
        }

        // Version
        if let Ok(ver) = conn.query("SELECT VERSION() as v").await {
            if let Some(v) = ver.rows.first().and_then(|r| r.get("v")) {
                cluster_info["doris_version"] = v.clone();
            }
        }

        // All non-default session vars
        if let Ok(vars) = conn
            .query("SELECT VARIABLE_NAME, VARIABLE_VALUE, DEFAULT_VALUE FROM information_schema.session_variables WHERE CHANGED = 1")
            .await
        {
            let all_vars: Vec<Value> = vars
                .rows
                .iter()
                .map(|r| {
                    let name = r.get("VARIABLE_NAME").and_then(|v| v.as_str()).unwrap_or("");
                    json!({
                        "name": name,
                        "value": r.get("VARIABLE_VALUE").and_then(|v| v.as_str()),
                        "default": r.get("DEFAULT_VALUE").and_then(|v| v.as_str()),
                        "impact": super::summary::classify_var_impact(name),
                    })
                })
                .collect();
            cluster_info["session_variables"] = Value::Array(all_vars);
        }

        cluster = cluster_info;
    }

    // ── Build full response ──
    let all_ops: Vec<Value> = flat_ops
        .iter()
        .map(|op| serde_json::to_value(op).unwrap_or(Value::Null))
        .collect();

    let mut result = json!({
        "profile": serde_json::to_value(&profile)?,
        "operators": all_ops,
    });

    if let Some(plan) = sections.get("Physical Plan") {
        result["physical_plan"] = Value::String(plan.clone());
    }
    if !table_context.is_empty() {
        result["scanned_tables"] = Value::Object(table_context);
    }
    if cluster != Value::Null {
        result["cluster"] = cluster;
    }

    // Provenance: which FE served this profile, how we got it, and what we tried.
    // Helpful for debugging multi-FE clusters where only one FE has the profile.
    if let Some(sb) = served_by {
        result["served_by"] = Value::String(sb);
    }
    if let Some(v) = via {
        result["fetch_via"] = Value::String(v.to_string());
    }
    if !attempts.is_empty() {
        let attempts_json: Vec<Value> = attempts.iter().map(|a| a.to_json()).collect();
        result["fetch_attempts"] = Value::Array(attempts_json);
    }

    Ok(result)
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}
