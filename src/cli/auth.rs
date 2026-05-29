use clap::{Args, Subcommand};

#[derive(Args)]
pub struct AuthCommand {
    #[command(subcommand)]
    pub action: AuthAction,
}

#[derive(Subcommand)]
pub enum AuthAction {
    /// Add a new environment
    Add(AddArgs),

    /// List all configured environments
    List,

    /// Test connection and show status
    Status,

    /// Remove an environment
    Remove(RemoveArgs),
}

#[derive(Args)]
pub struct AddArgs {
    /// Environment name
    pub name: String,

    /// MySQL connection URI (mysql://user:pass@host:port). Self-hosted only.
    #[arg(long)]
    pub mysql: Option<String>,

    /// Host address (self-hosted)
    #[arg(long)]
    pub host: Option<String>,

    /// MySQL port (self-hosted)
    #[arg(long, default_value = "9030")]
    pub port: u16,

    /// FE HTTP port (self-hosted)
    #[arg(long, default_value = "8030")]
    pub http_port: u16,

    /// Username (self-hosted)
    #[arg(long)]
    pub user: Option<String>,

    /// Password (self-hosted). `--mysql-password` is an alias.
    #[arg(long, alias = "mysql-password")]
    pub password: Option<String>,
}

#[derive(Args)]
pub struct RemoveArgs {
    /// Environment name to remove
    pub name: String,
}
