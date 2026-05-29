use crate::config::Environment;
use crate::connection::socks5_forwarder::Socks5Forwarder;
use crate::error::{DorisError, DorisResult};
use mysql_async::prelude::*;
use mysql_async::{Conn, Opts, OptsBuilder, Row, Value as MysqlValue};
use serde_json::{Map, Value};
pub struct MysqlConnection {
    conn: Conn,
    // Kept alive for the lifetime of the MySQL connection when SOCKS5 is in use.
    // Dropping it aborts the listener task; must outlive `conn`.
    #[allow(dead_code)]
    forwarder: Option<Socks5Forwarder>,
}

/// Result of a SQL query execution.
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Map<String, Value>>,
}

impl MysqlConnection {
    /// Connect to a Doris instance. Routes through SOCKS5 when `env.socks5`
    /// is set, via a loopback forwarder (mysql_async 0.34 has no native proxy hook).
    pub async fn connect(env: &Environment) -> DorisResult<Self> {
        let (dial_host, dial_port, forwarder, error_hint) = if let Some(s5) = &env.socks5 {
            let fwd = Socks5Forwarder::spawn(s5, env.host.clone(), env.mysql_port).await?;
            let port = fwd.local_addr.port();
            let hint = format!(" via socks5://{}:{}", s5.host, s5.port);
            ("127.0.0.1".to_string(), port, Some(fwd), hint)
        } else {
            (env.host.clone(), env.mysql_port, None, String::new())
        };

        let opts = OptsBuilder::default()
            .ip_or_hostname(&dial_host)
            .tcp_port(dial_port)
            .user(Some(&env.user))
            .pass(Some(&env.password))
            .prefer_socket(false);

        let pool = mysql_async::Pool::new(Opts::from(opts));
        let conn = pool.get_conn().await.map_err(|e| {
            DorisError::connection_with_source(
                format!(
                    "Failed to connect to {}:{} as '{}'{}",
                    env.host, env.mysql_port, env.user, error_hint
                ),
                e,
            )
        })?;

        let mut session = MysqlConnection { conn, forwarder };

        // Run the post-connect init directive (e.g. `USE @<compute-group>`),
        // sourced from `--init-sql` / `DORIS_INIT_SQL`. Never persisted.
        if let Some(directive) = &env.cluster_routing_directive {
            session.exec(directive).await?;
        }

        Ok(session)
    }

    /// Execute a SQL query and return structured results.
    pub async fn query(&mut self, sql: &str) -> DorisResult<QueryResult> {
        let rows: Vec<Row> = self
            .conn
            .query(sql)
            .await
            .map_err(|e| DorisError::sql(format!("{e}")))?;

        if rows.is_empty() {
            return Ok(QueryResult {
                columns: Vec::new(),
                rows: Vec::new(),
            });
        }

        // Extract column names from first row
        let columns: Vec<String> = rows[0]
            .columns_ref()
            .iter()
            .map(|c| c.name_str().to_string())
            .collect();

        let mut result_rows = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut map = Map::new();
            for (i, col_name) in columns.iter().enumerate() {
                let value = mysql_value_to_json(row.as_ref(i));
                map.insert(col_name.clone(), value);
            }
            result_rows.push(map);
        }

        Ok(QueryResult {
            columns,
            rows: result_rows,
        })
    }

    /// Execute a statement that doesn't return results (SET, USE, etc.).
    pub async fn exec(&mut self, sql: &str) -> DorisResult<()> {
        self.conn
            .query_drop(sql)
            .await
            .map_err(|e| DorisError::sql(format!("{e}")))?;
        Ok(())
    }

    /// Get the last query ID from the current session.
    pub async fn last_query_id(&mut self) -> DorisResult<String> {
        let result = self.query("SELECT last_query_id()").await?;
        if let Some(row) = result.rows.first() {
            if let Some(Value::String(qid)) = row.values().next() {
                return Ok(qid.clone());
            }
        }
        Ok(String::new())
    }

    /// Test connection by running SELECT 1, returns latency in ms.
    pub async fn ping(&mut self) -> DorisResult<u128> {
        let start = std::time::Instant::now();
        self.query("SELECT 1").await?;
        Ok(start.elapsed().as_millis())
    }
}

/// Convert a mysql_async Value to serde_json Value.
fn mysql_value_to_json(value: Option<&MysqlValue>) -> Value {
    match value {
        None | Some(MysqlValue::NULL) => Value::Null,
        Some(MysqlValue::Bytes(bytes)) => {
            let s = String::from_utf8_lossy(bytes).to_string();
            // Try to parse as number first
            if let Ok(n) = s.parse::<i64>() {
                Value::Number(n.into())
            } else if let Ok(n) = s.parse::<f64>() {
                serde_json::Number::from_f64(n)
                    .map(Value::Number)
                    .unwrap_or(Value::String(s))
            } else {
                Value::String(s)
            }
        }
        Some(MysqlValue::Int(n)) => Value::Number((*n).into()),
        Some(MysqlValue::UInt(n)) => Value::Number((*n).into()),
        Some(MysqlValue::Float(n)) => serde_json::Number::from_f64(*n as f64)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Some(MysqlValue::Double(n)) => serde_json::Number::from_f64(*n)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Some(other) => Value::String(format!("{other:?}")),
    }
}
