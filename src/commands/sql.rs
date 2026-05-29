use crate::cli::sql::SqlArgs;
use crate::config::Environment;
use crate::connection::MysqlConnection;
use serde_json::{json, Value};

pub async fn run(args: SqlArgs, env: &Environment) -> anyhow::Result<Value> {
    let sql = if let Some(query) = &args.query {
        query.clone()
    } else if let Some(file) = &args.file {
        std::fs::read_to_string(file)
            .map_err(|e| anyhow::anyhow!("Failed to read SQL file '{file}': {e}"))?
    } else {
        anyhow::bail!("Provide a SQL query or use -f <file.sql>");
    };

    // `env` carries any post-connect directive from `--init-sql` / DORIS_INIT_SQL
    // (e.g. the `USE @<compute-group>` handed off by `cloudcli cloud endpoint`);
    // MysqlConnection::connect applies it automatically.
    let mut conn = MysqlConnection::connect(env).await?;

    // Apply session variables
    if args.profile {
        conn.exec("SET enable_profile=true").await?;
    }
    if args.no_cache {
        conn.exec("SET enable_sql_cache=false").await?;
    }
    for var in &args.set_vars {
        conn.exec(&format!("SET {var}")).await?;
    }

    let start = std::time::Instant::now();
    let result = conn.query(&sql).await?;
    let exec_time_ms = start.elapsed().as_millis() as u64;

    let query_id = conn.last_query_id().await.unwrap_or_default();

    Ok(json!({
        "query_id": query_id,
        "exec_time_ms": exec_time_ms,
        "rows_returned": result.rows.len(),
        "columns": result.columns,
        "rows": result.rows,
    }))
}
