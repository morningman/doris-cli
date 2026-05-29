use clap::Args;

#[derive(Args)]
pub struct SqlArgs {
    /// SQL query to execute
    pub query: Option<String>,

    /// Execute SQL from file
    #[arg(short, long)]
    pub file: Option<String>,

    /// Enable query profiling (prepends SET enable_profile=true)
    #[arg(long)]
    pub profile: bool,

    /// Disable SQL cache for this query (for benchmarking)
    #[arg(long)]
    pub no_cache: bool,

    /// SET session variables before query (repeatable: --set "key=value")
    #[arg(long = "set")]
    pub set_vars: Vec<String>,
}
