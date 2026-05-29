use crate::config::environment::{Environment, EnvironmentConfig, EnvironmentCredentials};
use crate::error::{DorisError, DorisResult};
use crate::product::ProductProfile;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Top-level config file structure (<product-config-dir>/config.toml).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConfigFile {
    #[serde(default)]
    pub default_env: Option<String>,
    #[serde(default)]
    pub environments: BTreeMap<String, EnvironmentConfig>,
}

/// Top-level credentials file structure (<product-config-dir>/credentials.toml).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CredentialsFile {
    #[serde(flatten)]
    pub environments: BTreeMap<String, EnvironmentCredentials>,
}

/// The config store manages reading/writing config + credentials.
#[derive(Debug)]
pub struct Store {
    pub product: &'static ProductProfile,
    pub config: ConfigFile,
    pub credentials: CredentialsFile,
    config_dir: Option<PathBuf>,
}

impl Store {
    /// True when `<PREFIX>_HOST` + `<PREFIX>_USER` are set. In this mode the CLI must
    /// avoid all filesystem side effects — the bastion is multi-tenant and
    /// writable home dirs are not guaranteed.
    pub fn is_stateless(product: &ProductProfile) -> bool {
        std::env::var(product.env_key("HOST")).is_ok()
            && std::env::var(product.env_key("USER")).is_ok()
    }

    /// Build an in-memory Store with no backing files. `save()` refuses to
    /// write. Used when `is_stateless()` is true.
    pub fn stateless(product: &'static ProductProfile) -> Self {
        Store {
            product,
            config: ConfigFile::default(),
            credentials: CredentialsFile::default(),
            config_dir: None,
        }
    }

    /// Resolve an Environment purely from env vars. Never touches disk.
    pub fn resolve_stateless(product: &'static ProductProfile, name: &str) -> Environment {
        let host = std::env::var(product.env_key("HOST")).unwrap_or_default();
        let user = std::env::var(product.env_key("USER")).unwrap_or_default();
        let password = std::env::var(product.env_key("PASSWORD")).unwrap_or_default();
        let mysql_port = std::env::var(product.env_key("PORT"))
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(9030);
        let http_port = std::env::var(product.env_key("HTTP_PORT"))
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8030);
        Environment {
            product,
            name: name.to_string(),
            host,
            mysql_port,
            http_port,
            user,
            password,
            socks5: crate::config::Socks5Config::from_env(product),
            cluster_routing_directive: None,
        }
    }

    /// Load config from the product config directory (creates directory if needed).
    /// In stateless mode, returns an in-memory Store with no filesystem I/O.
    pub fn load(product: &'static ProductProfile) -> DorisResult<Self> {
        if Self::is_stateless(product) {
            return Ok(Self::stateless(product));
        }

        let config_dir = Self::config_dir(product)?;
        std::fs::create_dir_all(&config_dir)
            .map_err(|e| DorisError::config(format!("Failed to create config directory: {e}")))?;

        let config_path = config_dir.join("config.toml");
        let creds_path = config_dir.join("credentials.toml");

        let config: ConfigFile = if config_path.exists() {
            let text = std::fs::read_to_string(&config_path)
                .map_err(|e| DorisError::config(format!("Failed to read config: {e}")))?;
            toml::from_str(&text)
                .map_err(|e| DorisError::config(format!("Invalid config.toml: {e}")))?
        } else {
            ConfigFile::default()
        };

        let credentials: CredentialsFile = if creds_path.exists() {
            let text = std::fs::read_to_string(&creds_path)
                .map_err(|e| DorisError::config(format!("Failed to read credentials: {e}")))?;
            toml::from_str(&text)
                .map_err(|e| DorisError::config(format!("Invalid credentials.toml: {e}")))?
        } else {
            CredentialsFile::default()
        };

        Ok(Store {
            product,
            config,
            credentials,
            config_dir: Some(config_dir),
        })
    }

    /// Resolve an environment by name, applying env var overrides.
    pub fn resolve_env(&self, name: &str) -> DorisResult<Environment> {
        // Stateless shortcut — never touch config files.
        if Self::is_stateless(self.product) {
            return Ok(Self::resolve_stateless(self.product, name));
        }

        let env_config = self.config.environments.get(name).ok_or_else(|| {
            if self.config.environments.is_empty() {
                DorisError::AuthRequired
            } else {
                DorisError::EnvNotFound {
                    name: name.to_string(),
                }
            }
        })?;

        let creds = self
            .credentials
            .environments
            .get(name)
            .cloned()
            .unwrap_or_default();

        let env = Environment::from_config(self.product, name.to_string(), env_config, &creds);
        Ok(env.with_env_overrides())
    }

    /// Get the effective environment name (--env flag > <PREFIX>_ENV > config default).
    pub fn effective_env_name(&self, flag: &str) -> String {
        if flag != "default" {
            return flag.to_string();
        }
        if let Ok(env_name) = std::env::var(self.product.env_key("ENV")) {
            return env_name;
        }
        self.config
            .default_env
            .clone()
            .unwrap_or_else(|| "default".to_string())
    }

    /// Get the effective environment source for diagnostics.
    /// Returns (source_type, source_detail).
    pub fn effective_env_source(&self, flag: &str) -> (&'static str, String) {
        if flag != "default" {
            return ("flag", format!("--env {flag}"));
        }
        let env_key = self.product.env_key("ENV");
        if let Ok(name) = std::env::var(&env_key) {
            return ("env_var", format!("{env_key}={name}"));
        }
        if let Some(ref name) = self.config.default_env {
            return ("config", format!("default_env={name}"));
        }
        ("fallback", "no environment configured".to_string())
    }

    /// Add a new environment.
    pub fn add_env(
        &mut self,
        name: &str,
        config: EnvironmentConfig,
        creds: EnvironmentCredentials,
    ) -> DorisResult<()> {
        self.config.environments.insert(name.to_string(), config);
        self.credentials
            .environments
            .insert(name.to_string(), creds);

        // Set as default if it's the first environment
        if self.config.environments.len() == 1 {
            self.config.default_env = Some(name.to_string());
        }

        self.save()
    }

    /// Set the default environment.
    pub fn set_default(&mut self, name: &str) -> DorisResult<()> {
        if !self.config.environments.contains_key(name) {
            return Err(DorisError::EnvNotFound {
                name: name.to_string(),
            });
        }
        self.config.default_env = Some(name.to_string());
        self.save()
    }

    /// List all environment names with their configs.
    pub fn list_envs(&self) -> Vec<(String, &EnvironmentConfig, bool)> {
        let default_name = self.config.default_env.as_deref().unwrap_or("default");
        self.config
            .environments
            .iter()
            .map(|(name, config)| (name.clone(), config, name == default_name))
            .collect()
    }

    /// Remove an environment.
    pub fn remove_env(&mut self, name: &str) -> DorisResult<()> {
        self.config.environments.remove(name);
        self.credentials.environments.remove(name);
        if self.config.default_env.as_deref() == Some(name) {
            self.config.default_env = self.config.environments.keys().next().cloned();
        }
        self.save()
    }

    /// Save config and credentials to disk.
    fn save(&self) -> DorisResult<()> {
        let config_dir = self.config_dir.as_ref().ok_or_else(|| {
            DorisError::config(format!(
                "Cannot modify config in stateless mode ({} + {} set). \
                     Unset those env vars to use {}/ config files.",
                self.product.env_key("HOST"),
                self.product.env_key("USER"),
                self.product.config_dir,
            ))
        })?;

        let config_text = toml::to_string_pretty(&self.config)
            .map_err(|e| DorisError::config(format!("Failed to serialize config: {e}")))?;
        let creds_text = toml::to_string_pretty(&self.credentials)
            .map_err(|e| DorisError::config(format!("Failed to serialize credentials: {e}")))?;

        let config_path = config_dir.join("config.toml");
        let creds_path = config_dir.join("credentials.toml");

        std::fs::write(&config_path, config_text)
            .map_err(|e| DorisError::config(format!("Failed to write config: {e}")))?;
        std::fs::write(&creds_path, creds_text)
            .map_err(|e| DorisError::config(format!("Failed to write credentials: {e}")))?;

        // Set restrictive permissions on credentials file (Unix only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            let _ = std::fs::set_permissions(&creds_path, perms);
        }

        Ok(())
    }

    fn config_dir(product: &ProductProfile) -> DorisResult<PathBuf> {
        let home =
            dirs::home_dir().ok_or_else(|| DorisError::config("Cannot determine home directory"))?;
        Ok(home.join(&product.config_dir))
    }
}
