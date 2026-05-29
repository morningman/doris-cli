use crate::config::Environment;
use crate::connection::MysqlConnection;
use serde_json::{json, Value};

pub async fn run(limit: usize, env: &Environment) -> anyhow::Result<Value> {
    let mut conn = MysqlConnection::connect(env).await?;

    let result = conn.query("SHOW QUERY PROFILE '/'").await?;

    let profiles: Vec<Value> = result
        .rows
        .iter()
        .take(limit)
        .map(|row| {
            let sql = row
                .get("Sql Statement")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            // Truncate SQL for display
            let sql_truncated = if sql.len() > 120 {
                format!("{}...", &sql[..120])
            } else {
                sql.to_string()
            };

            // Clean up escaped newlines and collapse whitespace
            let sql_clean = sql_truncated
                .replace("\\n", " ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");

            json!({
                "query_id": row.get("Profile ID").and_then(|v| v.as_str()).unwrap_or(""),
                "sql": sql_clean,
                "total_time": row.get("Total").and_then(|v| v.as_str()).unwrap_or(""),
                "state": row.get("Task State").and_then(|v| v.as_str()).unwrap_or(""),
                "user": row.get("User").and_then(|v| v.as_str()).unwrap_or(""),
                "start_time": row.get("Start Time").and_then(|v| v.as_str()).unwrap_or(""),
                "default_db": row.get("Default Db").and_then(|v| v.as_str()).unwrap_or(""),
            })
        })
        .collect();

    Ok(Value::Array(profiles))
}

/// List currently running queries via information_schema.active_queries.
pub async fn run_active(env: &Environment) -> anyhow::Result<Value> {
    let mut conn = MysqlConnection::connect(env).await?;

    let result = conn
        .query(
            "SELECT * FROM information_schema.active_queries \
             ORDER BY QUERY_TIME_MS DESC",
        )
        .await?;

    let queries: Vec<Value> = result
        .rows
        .iter()
        .map(|row| {
            let sql = row.get("SQL").and_then(|v| v.as_str()).unwrap_or("");
            let sql_truncated = if sql.len() > 120 {
                format!("{}...", &sql[..120])
            } else {
                sql.to_string()
            };
            let sql_clean = sql_truncated
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");

            json!({
                "query_id": row.get("QUERY_ID").and_then(|v| v.as_str()).unwrap_or(""),
                "sql": sql_clean,
                "running_ms": row.get("QUERY_TIME_MS").and_then(|v| v.as_u64()),
                "user": row.get("USER").and_then(|v| v.as_str()).unwrap_or(""),
                "database": row.get("DATABASE").and_then(|v| v.as_str()).unwrap_or(""),
                "start_time": row.get("QUERY_START_TIME").and_then(|v| v.as_str()).unwrap_or(""),
                "workload_group_id": row.get("WORKLOAD_GROUP_ID").and_then(|v| v.as_u64()),
            })
        })
        .collect();

    Ok(Value::Array(queries))
}
