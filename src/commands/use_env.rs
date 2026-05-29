use crate::cli::root::UseArgs;
use crate::config::Store;
use serde_json::{json, Value};

pub fn run(args: UseArgs, store: &mut Store) -> anyhow::Result<Value> {
    match args.name {
        Some(name) => switch(store, &name),
        None => show_current(store),
    }
}

fn switch(store: &mut Store, name: &str) -> anyhow::Result<Value> {
    // Extract config details before mutating store
    let details = store
        .list_envs()
        .iter()
        .find(|(n, _, _)| n == name)
        .map(|(_, config, _)| (config.host.clone(), config.user.clone()));

    store.set_default(name)?;

    let mut result = json!({
        "status": "switched",
        "environment": name,
    });

    if let Some((host, user)) = details {
        result["host"] = json!(host);
        result["user"] = json!(user);
        result["message"] = json!(format!("Switched to: {} ({} as {})", name, host, user));
    }

    Ok(result)
}

fn show_current(store: &Store) -> anyhow::Result<Value> {
    let envs = store.list_envs();
    let current = store.effective_env_name("default");

    let list: Vec<Value> = envs
        .iter()
        .map(|(name, config, _)| {
            json!({
                "name": name,
                "host": config.host,
                "user": config.user,
                "active": name == &current,
            })
        })
        .collect();

    if list.is_empty() {
        Ok(json!({
            "current": null,
            "environments": [],
            "message": format!(
                "No environments configured. Run `{} auth add <name> --host <host> --user <user> --password <pass>` to get started.",
                store.product.binary
            )
        }))
    } else {
        Ok(json!({
            "current": current,
            "environments": list,
        }))
    }
}
