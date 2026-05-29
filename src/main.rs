mod cli;
mod commands;
mod config;
mod connection;
mod error;
mod models;
mod output;
mod parser;
mod product;

use crate::cli::{Cli, Command};
use crate::product::{get_product, ProductProfile};

#[tokio::main]
async fn main() {
    main_for(get_product("doris")).await;
}

async fn main_for(product: &'static ProductProfile) {
    let cli = Cli::parse_for(product);

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::WARN.into()),
        )
        .with_target(false)
        .without_time()
        .init();

    let format = crate::output::format::resolve(cli.format);
    let result = run_command(cli, product).await;

    match result {
        Ok(value) => {
            if let Err(e) = crate::output::render(&value, format) {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        Err(e) => {
            if let Some(doris_err) = e.downcast_ref::<crate::error::DorisError>() {
                eprintln!("{}", doris_err.user_message(product));
            } else {
                eprintln!("Error: {e}");
            }
            std::process::exit(1);
        }
    }
}

async fn run_command(
    cli: Cli,
    product: &'static ProductProfile,
) -> anyhow::Result<serde_json::Value> {
    let socks5_flag = match cli.socks5.as_deref() {
        Some(s) => Some(crate::config::Socks5Config::parse_flag(s)?),
        None => None,
    };

    // `--init-sql` (flag) > DORIS_INIT_SQL (env) becomes the session's
    // post-connect directive — e.g. `USE @<compute-group>` handed off from
    // `cloudcli cloud endpoint --export`. MysqlConnection::connect applies it.
    let init_sql = cli
        .init_sql
        .clone()
        .or_else(|| std::env::var(product.env_key("INIT_SQL")).ok());

    let apply = |mut env: crate::config::Environment| {
        if socks5_flag.is_some() {
            env.socks5 = socks5_flag.clone();
        }
        if init_sql.is_some() {
            env.cluster_routing_directive = init_sql.clone();
        }
        env
    };

    match cli.command {
        Command::Use(args) => {
            let mut store = crate::config::Store::load(product)?;
            crate::commands::use_env::run(args, &mut store)
        }
        Command::Auth(cmd) => {
            let mut store = crate::config::Store::load(product)?;
            crate::commands::auth::run(cmd, &mut store, &cli.env).await
        }
        Command::Sql(args) => {
            let store = crate::config::Store::load(product)?;
            let env_name = store.effective_env_name(&cli.env);
            let env = apply(store.resolve_env(&env_name)?);
            crate::commands::sql::run(args, &env).await
        }
        Command::Tablet(args) => {
            let store = crate::config::Store::load(product)?;
            let env_name = store.effective_env_name(&cli.env);
            let env = apply(store.resolve_env(&env_name)?);
            crate::commands::tablet::run(args, &env).await
        }
        Command::Profile(cmd) => {
            let store = crate::config::Store::load(product)?;
            let env_name = store.effective_env_name(&cli.env);
            let env = apply(store.resolve_env(&env_name)?);
            crate::commands::profile::run(cmd, &env).await
        }
    }
}
