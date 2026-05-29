use crate::output::format::OutputFormatArg;
use crate::product::ProductProfile;
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};

#[derive(Parser)]
#[command(version, propagate_version = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Authentication environment to use
    #[arg(long, global = true, default_value = "default")]
    pub env: String,

    /// Output format (auto-detects: json when piped, table when TTY)
    #[arg(long, global = true, value_enum)]
    pub format: Option<OutputFormatArg>,

    /// SOCKS5 proxy in "user:pass@host:port" form (BYOC). Overrides <PREFIX>_SOCKS5_*.
    #[arg(long, global = true)]
    pub socks5: Option<String>,

    /// SQL to run immediately after connecting (e.g. `USE @<compute-group>`).
    /// Overrides DORIS_INIT_SQL. Used for the `cloudcli cloud endpoint` handoff.
    #[arg(long, global = true)]
    pub init_sql: Option<String>,
}

impl Cli {
    pub fn parse_for(product: &'static ProductProfile) -> Self {
        let matches = Self::command()
            .name(product.binary.as_str())
            .bin_name(product.binary.as_str())
            .about(product.about.as_str())
            .get_matches();
        Self::from_arg_matches(&matches).unwrap_or_else(|e| e.exit())
    }
}

#[derive(Args)]
pub struct UseArgs {
    /// Environment name to switch to (omit to show current)
    pub name: Option<String>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Switch active environment
    Use(UseArgs),

    /// Manage authentication environments
    Auth(super::auth::AuthCommand),

    /// Execute SQL queries
    Sql(super::sql::SqlArgs),

    /// Tablet and bucket analysis
    Tablet(super::tablet::TabletArgs),

    /// Query profile analysis
    Profile(super::profile::ProfileCommand),
}
