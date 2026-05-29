use clap::Args;

#[derive(Args)]
pub struct TabletArgs {
    /// Table name (e.g., db.table or just table)
    pub table: String,

    /// Show detailed distribution: per-partition stats + per-tablet + backend mapping
    #[arg(long)]
    pub detail: bool,

    /// Filter by partition name (used with --detail)
    #[arg(long)]
    pub partition: Option<String>,
}
