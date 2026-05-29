use crate::product::ProductProfile;

#[derive(Debug, thiserror::Error)]
pub enum VeloError {
    #[error("Connection failed: {message}")]
    Connection {
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },

    #[error("SQL error: {message}")]
    Sql { message: String },

    #[error("Profile parse error: {0}")]
    #[allow(dead_code)]
    Parse(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("HTTP error [{status}]: {body}")]
    Http { status: u16, body: String },

    #[error("Environment '{name}' not found.")]
    EnvNotFound { name: String },

    #[error("Authentication required.")]
    AuthRequired,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl VeloError {
    pub fn connection_with_source(
        msg: impl Into<String>,
        src: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        VeloError::Connection {
            message: msg.into(),
            source: Some(Box::new(src)),
        }
    }

    pub fn sql(msg: impl Into<String>) -> Self {
        VeloError::Sql {
            message: msg.into(),
        }
    }

    pub fn config(msg: impl Into<String>) -> Self {
        VeloError::Config(msg.into())
    }

    #[allow(dead_code)]
    pub fn parse(msg: impl Into<String>) -> Self {
        VeloError::Parse(msg.into())
    }
}

/// Result type alias for VeloError
pub type VeloResult<T> = Result<T, VeloError>;

/// Format a VeloError for user-facing display
impl VeloError {
    pub fn user_message(&self, product: &ProductProfile) -> String {
        match self {
            VeloError::Connection { message, .. } => {
                format!(
                    "Error: {message}\n\nCheck your connection settings with `{} auth status`.",
                    product.binary
                )
            }
            VeloError::EnvNotFound { name } => {
                format!(
                    "Error: Environment '{name}' not found.\n\nRun `{} auth list` to see available environments.",
                    product.binary
                )
            }
            VeloError::AuthRequired => {
                format!(
                    "Error: No authentication configured.\n\nRun `{} auth add <name> --host <host> --user <user> --password <pass>` to get started.",
                    product.binary
                )
            }
            other => format!("Error: {other}"),
        }
    }
}
