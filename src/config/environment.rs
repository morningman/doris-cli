use crate::error::{VeloError, VeloResult};
use crate::product::ProductProfile;
use serde::{Deserialize, Serialize};

/// A resolved environment with all connection details ready to use.
#[derive(Debug, Clone)]
pub struct Environment {
    pub product: &'static ProductProfile,
    /// The resolved environment's name (kept as metadata; not all paths read it).
    #[allow(dead_code)]
    pub name: String,
    pub host: String,
    pub mysql_port: u16,
    pub http_port: u16,
    pub user: String,
    pub password: String,
    pub socks5: Option<Socks5Config>,
    /// Runtime-only (never persisted): a SQL statement to issue immediately
    /// after `MysqlConnection::connect` succeeds — e.g. `USE @<compute-group>`
    /// sourced from `--init-sql` / `DORIS_INIT_SQL` (the `cloudcli cloud
    /// endpoint` handoff).
    pub cluster_routing_directive: Option<String>,
}

/// Stored environment config (without secrets).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentConfig {
    pub host: String,
    #[serde(default = "default_mysql_port")]
    pub mysql_port: u16,
    #[serde(default = "default_http_port")]
    pub http_port: u16,
    pub user: String,
}

/// Stored credentials (secrets).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EnvironmentCredentials {
    #[serde(default)]
    pub password: String,
}

/// Runtime-only SOCKS5 proxy configuration.
///
/// Never persisted to disk. Sourced from the `--socks5` flag or `<PREFIX>_SOCKS5_*`
/// env vars because BYOC bastions are multi-tenant — one tenant's credentials
/// must not leak to another through a shared config file.
#[derive(Debug, Clone)]
pub struct Socks5Config {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub pass: String,
}

impl Socks5Config {
    /// Build from env vars. Returns None unless both host and port are set.
    /// user/pass default to "admin"/"admin" per VeloDB BYOC convention.
    pub fn from_env(product: &ProductProfile) -> Option<Self> {
        let host = std::env::var(product.env_key("SOCKS5_HOST")).ok()?;
        let port: u16 = std::env::var(product.env_key("SOCKS5_PORT"))
            .ok()?
            .parse()
            .ok()?;
        let user =
            std::env::var(product.env_key("SOCKS5_USER")).unwrap_or_else(|_| "admin".to_string());
        let pass =
            std::env::var(product.env_key("SOCKS5_PASS")).unwrap_or_else(|_| "admin".to_string());
        Some(Socks5Config {
            host,
            port,
            user,
            pass,
        })
    }

    /// Parse `user:pass@host:port` (matches the Go mysql-client reference).
    pub fn parse_flag(s: &str) -> VeloResult<Self> {
        let (auth, hostport) = s.split_once('@').ok_or_else(|| {
            VeloError::config(format!(
                "--socks5 must be in 'user:pass@host:port' form, got '{s}'"
            ))
        })?;
        let (user, pass) = auth.split_once(':').ok_or_else(|| {
            VeloError::config(format!("--socks5 auth must be 'user:pass', got '{auth}'"))
        })?;
        let (host, port_s) = hostport.rsplit_once(':').ok_or_else(|| {
            VeloError::config(format!(
                "--socks5 host must be 'host:port', got '{hostport}'"
            ))
        })?;
        let port: u16 = port_s.parse().map_err(|_| {
            VeloError::config(format!("--socks5 port '{port_s}' is not a valid u16"))
        })?;
        Ok(Socks5Config {
            host: host.to_string(),
            port,
            user: user.to_string(),
            pass: pass.to_string(),
        })
    }
}

fn default_mysql_port() -> u16 {
    9030
}

fn default_http_port() -> u16 {
    8030
}

impl Environment {
    /// Apply environment variable overrides (highest precedence before the --socks5 flag).
    pub fn with_env_overrides(mut self) -> Self {
        if let Ok(host) = std::env::var(self.product.env_key("HOST")) {
            self.host = host;
        }
        if let Ok(user) = std::env::var(self.product.env_key("USER")) {
            self.user = user;
        }
        if let Ok(password) = std::env::var(self.product.env_key("PASSWORD")) {
            self.password = password;
        }
        if let Ok(port) = std::env::var(self.product.env_key("PORT")) {
            if let Ok(p) = port.parse() {
                self.mysql_port = p;
            }
        }
        if let Ok(port) = std::env::var(self.product.env_key("HTTP_PORT")) {
            if let Ok(p) = port.parse() {
                self.http_port = p;
            }
        }
        if self.socks5.is_none() {
            self.socks5 = Socks5Config::from_env(self.product);
        }
        self
    }

    /// Build from config + credentials.
    pub fn from_config(
        product: &'static ProductProfile,
        name: String,
        config: &EnvironmentConfig,
        creds: &EnvironmentCredentials,
    ) -> Self {
        Environment {
            product,
            name,
            host: config.host.clone(),
            mysql_port: config.mysql_port,
            http_port: config.http_port,
            user: config.user.clone(),
            password: creds.password.clone(),
            socks5: None,
            cluster_routing_directive: None,
        }
    }
}
