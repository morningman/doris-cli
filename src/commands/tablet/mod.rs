pub mod detail;
pub mod overview;

use crate::cli::tablet::TabletArgs;
use crate::config::Environment;
use serde_json::Value;

pub async fn run(args: TabletArgs, env: &Environment) -> anyhow::Result<Value> {
    if args.detail {
        detail::run(&args.table, args.partition.as_deref(), env).await
    } else {
        overview::run(&args.table, env).await
    }
}
