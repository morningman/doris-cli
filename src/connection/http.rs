use crate::config::Environment;
use crate::error::{DorisError, DorisResult};
use serde_json::Value;

pub struct HttpConnection {
    client: reqwest::Client,
    base_url: String,
    user: String,
    password: String,
}

/// Outcome of probing a single HTTP endpoint.
#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub url: String,
    pub status: ProbeStatus,
}

#[derive(Debug, Clone)]
pub enum ProbeStatus {
    /// 2xx response — endpoint is reachable and auth succeeded.
    Ok,
    /// Non-2xx HTTP response — port is open, but endpoint/auth is off.
    Http(u16),
    /// Network/timeout/DNS failure — port is likely wrong or service down.
    Unreachable(String),
}

impl ProbeStatus {
    pub fn is_ok(&self) -> bool {
        matches!(self, ProbeStatus::Ok)
    }

    pub fn short(&self) -> String {
        match self {
            ProbeStatus::Ok => "ok".to_string(),
            ProbeStatus::Http(c) => format!("http {c}"),
            ProbeStatus::Unreachable(e) => format!("unreachable: {e}"),
        }
    }
}

/// Aggregate HTTP health of the configured FE.
#[derive(Debug, Clone)]
pub struct HttpProbe {
    pub rest_v2: ProbeResult,
    pub legacy: ProbeResult,
}

impl HttpProbe {
    pub fn any_ok(&self) -> bool {
        self.rest_v2.status.is_ok() || self.legacy.status.is_ok()
    }
}

impl HttpConnection {
    pub fn new(env: &Environment) -> Self {
        let mut builder = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(5));

        if let Some(s5) = &env.socks5 {
            // socks5h:// → DNS resolved on the proxy side (target hostnames may
            // only exist inside the customer VPC in BYOC).
            let proxy_url = format!("socks5h://{}:{}", s5.host, s5.port);
            match reqwest::Proxy::all(&proxy_url) {
                Ok(p) => {
                    builder = builder.proxy(p.basic_auth(&s5.user, &s5.pass));
                }
                Err(e) => {
                    tracing::warn!(
                        target = "doris::socks5",
                        "Invalid SOCKS5 proxy URL '{proxy_url}': {e} — HTTP will go direct"
                    );
                }
            }
        }

        HttpConnection {
            client: builder.build().unwrap_or_default(),
            base_url: format!("http://{}:{}", env.host, env.http_port),
            user: env.user.clone(),
            password: env.password.clone(),
        }
    }

    /// Build a connection for a specific host:port, reusing env's credentials + socks5.
    /// Used when fanning out to other FEs (via SHOW FRONTENDS) or probing alternative ports.
    pub fn for_target(env: &Environment, host: &str, http_port: u16) -> Self {
        let mut builder = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(5));

        if let Some(s5) = &env.socks5 {
            let proxy_url = format!("socks5h://{}:{}", s5.host, s5.port);
            if let Ok(p) = reqwest::Proxy::all(&proxy_url) {
                builder = builder.proxy(p.basic_auth(&s5.user, &s5.pass));
            }
        }

        HttpConnection {
            client: builder.build().unwrap_or_default(),
            base_url: format!("http://{host}:{http_port}"),
            user: env.user.clone(),
            password: env.password.clone(),
        }
    }

    /// Probe REST v2 + legacy HTTP endpoints with a short timeout.
    /// Used by `auth add` / `auth status` to surface connectivity issues
    /// early, rather than letting them explode later inside `profile get`.
    pub async fn probe(&self) -> HttpProbe {
        let rest_v2_url = format!(
            "{}/rest/v2/manager/query/query_info?is_all_node=true",
            self.base_url
        );
        let legacy_url = format!("{}/api/health", self.base_url);

        let rest_v2 = self.probe_one(&rest_v2_url).await;
        let legacy = self.probe_one(&legacy_url).await;

        HttpProbe { rest_v2, legacy }
    }

    async fn probe_one(&self, url: &str) -> ProbeResult {
        let resp = self
            .client
            .get(url)
            .basic_auth(&self.user, Some(&self.password))
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await;

        let status = match resp {
            Ok(r) => {
                let code = r.status().as_u16();
                if (200..300).contains(&code) {
                    ProbeStatus::Ok
                } else {
                    ProbeStatus::Http(code)
                }
            }
            Err(e) => ProbeStatus::Unreachable(short_err(&e)),
        };

        ProbeResult {
            url: url.to_string(),
            status,
        }
    }
}

/// Collapse reqwest's verbose error chain into one short line for probe output.
fn short_err(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timeout".to_string()
    } else if e.is_connect() {
        "connection refused".to_string()
    } else if e.is_request() {
        "request failed".to_string()
    } else {
        e.to_string().chars().take(120).collect()
    }
}

impl HttpConnection {
    /// Fetch profile text via REST v2 Manager API (works on cloud port 8080).
    /// Endpoint: GET /rest/v2/manager/query/profile/text/{query_id}?is_all_node=true
    /// Response: {"msg":"success","code":0,"data":{"profile":"Summary:\n..."}}
    pub async fn get_profile_text_v2(&self, query_id: &str) -> DorisResult<String> {
        let url = format!(
            "{}/rest/v2/manager/query/profile/text/{}?is_all_node=true",
            self.base_url, query_id
        );

        let resp = self
            .client
            .get(&url)
            .basic_auth(&self.user, Some(&self.password))
            .send()
            .await
            .map_err(|e| {
                DorisError::connection_with_source(
                    format!("REST v2 profile request failed: {url}"),
                    e,
                )
            })?;

        let status = resp.status().as_u16();
        if status != 200 {
            let body = resp.text().await.unwrap_or_default();
            return Err(DorisError::Http { status, body });
        }

        let json: Value = resp.json().await.map_err(|e| DorisError::Http {
            status,
            body: format!("Failed to parse JSON response: {e}"),
        })?;

        // Check response envelope
        let code = json.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            let msg = json
                .get("data")
                .and_then(|v| v.as_str())
                .or_else(|| json.get("msg").and_then(|v| v.as_str()))
                .unwrap_or("Unknown error");
            return Err(DorisError::Http {
                status,
                body: msg.to_string(),
            });
        }

        // Extract profile text from data.profile
        json.get("data")
            .and_then(|d| d.get("profile"))
            .and_then(|p| p.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| DorisError::Parse("No 'profile' field in REST v2 response".to_string()))
    }

    /// Fetch profile text from legacy FE HTTP API.
    /// Endpoint: GET /api/profile/text/{query_id}
    pub async fn get_profile_text(&self, query_id: &str) -> DorisResult<String> {
        let url = format!("{}/api/profile/text/{}", self.base_url, query_id);
        let resp = self
            .client
            .get(&url)
            .basic_auth(&self.user, Some(&self.password))
            .send()
            .await
            .map_err(|e| {
                DorisError::connection_with_source(format!("HTTP request failed: {url}"), e)
            })?;

        let status = resp.status().as_u16();
        let body = resp.text().await.map_err(|e| DorisError::Http {
            status,
            body: e.to_string(),
        })?;

        if status != 200 {
            return Err(DorisError::Http { status, body });
        }

        Ok(body)
    }

    /// Fetch list of recent queries via REST v2 Manager API.
    /// Endpoint: GET /rest/v2/manager/query/query_info?is_all_node=true
    #[allow(dead_code)]
    pub async fn get_query_list(&self) -> DorisResult<Value> {
        let url = format!(
            "{}/rest/v2/manager/query/query_info?is_all_node=true",
            self.base_url
        );

        let resp = self
            .client
            .get(&url)
            .basic_auth(&self.user, Some(&self.password))
            .send()
            .await
            .map_err(|e| {
                DorisError::connection_with_source(format!("REST v2 query list failed: {url}"), e)
            })?;

        let json: Value = resp.json().await.map_err(|e| DorisError::Http {
            status: 0,
            body: format!("Failed to parse JSON: {e}"),
        })?;

        Ok(json)
    }

    /// Test HTTP connectivity.
    #[allow(dead_code)]
    pub async fn ping(&self) -> DorisResult<u128> {
        let start = std::time::Instant::now();
        let url = format!("{}/api/health", self.base_url);
        let _ = self
            .client
            .get(&url)
            .basic_auth(&self.user, Some(&self.password))
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await;
        Ok(start.elapsed().as_millis())
    }
}
