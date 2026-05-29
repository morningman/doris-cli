pub mod diff;
pub mod fetch;
pub mod full;
pub mod history;
pub mod list;
pub mod summary;

use crate::cli::profile::{ProfileAction, ProfileCommand};
use crate::config::Environment;
use serde_json::Value;

pub async fn run(cmd: ProfileCommand, env: &Environment) -> anyhow::Result<Value> {
    match cmd.action {
        ProfileAction::List(args) => {
            if args.active {
                list::run_active(env).await
            } else {
                list::run(args.limit, env).await
            }
        }
        ProfileAction::Get(args) => {
            let profile_source = if let Some(file) = &args.file {
                Some(std::fs::read_to_string(file).map_err(|e| {
                    anyhow::anyhow!("Failed to read profile file '{file}': {e}")
                })?)
            } else {
                None
            };

            if args.raw {
                if let Some(text) = profile_source {
                    Ok(Value::String(text))
                } else {
                    let fetched = fetch::fetch_profile_text(&args.query_id, env)
                        .await
                        .map_err(|e| anyhow::anyhow!("{e}"))?;
                    Ok(Value::String(fetched.text))
                }
            } else if args.full {
                full::run(&args.query_id, profile_source.as_deref(), env).await
            } else {
                // Default: summary + ALL operators + DDL + fragments + health
                summary::run(&args.query_id, profile_source.as_deref(), env).await
            }
        }
        ProfileAction::Diff(args) => diff::run(&args.slow_qid, &args.fast_qid, env).await,
        ProfileAction::History(args) => {
            history::run(&args.sql_pattern, args.days, args.limit, env).await
        }
    }
}
