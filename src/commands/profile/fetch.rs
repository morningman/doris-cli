use crate::config::Environment;
use crate::connection::{HttpConnection, MysqlConnection};
use serde_json::{json, Value};
use std::fmt;

/// One attempt to fetch a profile via a specific API.
/// Collected so `profile get` can tell the user *why* a fetch failed instead
/// of swallowing errors behind a generic "not accessible" note.
#[derive(Debug, Clone)]
pub struct FetchAttempt {
    pub target: String,    // "host:port"
    pub via: &'static str, // "rest_v2" | "legacy"
    pub result: String,    // "ok" or error description
}

impl FetchAttempt {
    pub fn to_json(&self) -> Value {
        json!({
            "target": self.target,
            "via": self.via,
            "result": self.result,
        })
    }
}

/// Successful profile fetch — text plus metadata about how we got it.
pub struct ProfileFetch {
    pub text: String,
    pub served_by: String,
    pub via: &'static str,
    pub attempts: Vec<FetchAttempt>,
}

/// Failure across all APIs attempted. Carries the full attempt log.
#[derive(Debug)]
pub struct FetchFailure {
    pub query_id: String,
    pub binary: String,
    pub attempts: Vec<FetchAttempt>,
}

impl FetchFailure {
    pub fn attempts_json(&self) -> Value {
        Value::Array(self.attempts.iter().map(FetchAttempt::to_json).collect())
    }
}

impl fmt::Display for FetchFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Could not fetch profile '{}'.\n\
             REST v2 with is_all_node=true aggregates profiles across FEs server-side,\n\
             so this usually means the HTTP port is wrong or the profile has been evicted.\n\
             Cloud deployments use --http-port 8080; self-hosted Doris uses 8030.\n\
             Run `{} auth status` to check connectivity.\n\
             Attempts:\n{}",
            self.query_id,
            self.binary,
            self.attempts
                .iter()
                .map(|a| format!("  - {} via {}: {}", a.target, a.via, a.result))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}

impl std::error::Error for FetchFailure {}

/// Fetch profile text from the configured FE.
///
/// Tries REST v2 first (which server-side aggregates across FEs via
/// `is_all_node=true`), then falls back to the legacy `/api/profile/text/`
/// endpoint for older Doris versions. Every attempt is recorded so the
/// caller can surface a specific reason for failure.
pub async fn fetch_profile_text(
    query_id: &str,
    env: &Environment,
) -> Result<ProfileFetch, FetchFailure> {
    let mut attempts: Vec<FetchAttempt> = Vec::new();
    let target = format!("{}:{}", env.host, env.http_port);
    let conn = HttpConnection::new(env);

    if let Some((text, via)) = try_fetch(&conn, &target, query_id, &mut attempts).await {
        return Ok(ProfileFetch {
            text,
            served_by: target,
            via,
            attempts,
        });
    }

    Err(FetchFailure {
        query_id: query_id.to_string(),
        binary: env.product.binary.clone(),
        attempts,
    })
}

/// Try REST v2 then legacy on one connection. Returns (text, via) on first success.
async fn try_fetch(
    conn: &HttpConnection,
    target: &str,
    query_id: &str,
    attempts: &mut Vec<FetchAttempt>,
) -> Option<(String, &'static str)> {
    match conn.get_profile_text_v2(query_id).await {
        Ok(text) if !text.is_empty() && text.contains("Summary") => {
            attempts.push(FetchAttempt {
                target: target.to_string(),
                via: "rest_v2",
                result: "ok".to_string(),
            });
            return Some((text, "rest_v2"));
        }
        Ok(_) => {
            attempts.push(FetchAttempt {
                target: target.to_string(),
                via: "rest_v2",
                result: "empty or malformed response".to_string(),
            });
        }
        Err(e) => {
            attempts.push(FetchAttempt {
                target: target.to_string(),
                via: "rest_v2",
                result: format!("{e}"),
            });
        }
    }

    match conn.get_profile_text(query_id).await {
        Ok(text) if !text.is_empty() && text.contains("Summary") => {
            attempts.push(FetchAttempt {
                target: target.to_string(),
                via: "legacy",
                result: "ok".to_string(),
            });
            return Some((text, "legacy"));
        }
        Ok(_) => {
            attempts.push(FetchAttempt {
                target: target.to_string(),
                via: "legacy",
                result: "empty or non-profile body".to_string(),
            });
        }
        Err(e) => {
            attempts.push(FetchAttempt {
                target: target.to_string(),
                via: "legacy",
                result: format!("{e}"),
            });
        }
    }

    None
}

/// Build a basic summary from SHOW QUERY PROFILE list metadata.
/// This works even when the full profile text isn't accessible.
pub async fn fetch_summary_from_sql(
    query_id: &str,
    env: &Environment,
) -> anyhow::Result<Option<Value>> {
    let mut conn = MysqlConnection::connect(env).await?;
    let result = conn.query("SHOW QUERY PROFILE '/'").await?;

    for row in &result.rows {
        let pid = row.get("Profile ID").and_then(|v| v.as_str()).unwrap_or("");

        if pid == query_id {
            return Ok(Some(json!({
                "query_id": pid,
                "sql": row.get("Sql Statement").and_then(|v| v.as_str()).unwrap_or(""),
                "total_time": row.get("Total").and_then(|v| v.as_str()).unwrap_or(""),
                "state": row.get("Task State").and_then(|v| v.as_str()).unwrap_or(""),
                "user": row.get("User").and_then(|v| v.as_str()).unwrap_or(""),
                "start_time": row.get("Start Time").and_then(|v| v.as_str()).unwrap_or(""),
                "end_time": row.get("End Time").and_then(|v| v.as_str()).unwrap_or(""),
                "default_db": row.get("Default Db").and_then(|v| v.as_str()).unwrap_or(""),
            })));
        }
    }

    Ok(None)
}
