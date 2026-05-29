use clap::{Args, Subcommand};

#[derive(Args)]
pub struct ProfileCommand {
    #[command(subcommand)]
    pub action: ProfileAction,
}

#[derive(Subcommand)]
pub enum ProfileAction {
    /// List recent query profiles
    List(ProfileListArgs),

    /// Get a specific query profile (default: summary + plan + operators + table context)
    Get(ProfileGetArgs),

    /// Compare two query profiles (slow vs fast run)
    Diff(ProfileDiffArgs),

    /// Show execution history for a query pattern (from audit_log)
    History(ProfileHistoryArgs),
}

#[derive(Args)]
pub struct ProfileListArgs {
    /// Maximum number of profiles to return
    #[arg(long, default_value = "20")]
    pub limit: usize,

    /// Show currently running queries (from information_schema.active_queries)
    #[arg(long)]
    pub active: bool,
}

#[derive(Args)]
pub struct ProfileGetArgs {
    /// Query ID
    pub query_id: String,

    /// Show full parsed tree (Fragment → Pipeline → Operator with all counters)
    #[arg(long)]
    pub full: bool,

    /// Return raw unprocessed profile text
    #[arg(long)]
    pub raw: bool,

    /// Load profile text from file (e.g., exported from web UI)
    #[arg(short, long)]
    pub file: Option<String>,
}

#[derive(Args)]
pub struct ProfileDiffArgs {
    /// Query ID of the slow run
    pub slow_qid: String,

    /// Query ID of the fast run
    pub fast_qid: String,
}

#[derive(Args)]
pub struct ProfileHistoryArgs {
    /// SQL text or substring to match in audit_log
    pub sql_pattern: String,

    /// Number of days to look back
    #[arg(long, default_value = "7")]
    pub days: u32,

    /// Maximum number of entries
    #[arg(long, default_value = "50")]
    pub limit: usize,
}
