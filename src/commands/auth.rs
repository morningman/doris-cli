use crate::cli::auth::{AddArgs, AuthAction, AuthCommand};
use crate::config::environment::{EnvironmentConfig, EnvironmentCredentials};
use crate::config::Store;
use crate::connection::{HttpConnection, HttpProbe, MysqlConnection, ProbeStatus};
use serde_json::{json, Value};

pub async fn run(cmd: AuthCommand, store: &mut Store, env_flag: &str) -> anyhow::Result<Value> {
    match cmd.action {
        AuthAction::Add(args) => run_add(args, store).await,
        AuthAction::List => run_list(store),
        AuthAction::Status => run_status(store, env_flag).await,
        AuthAction::Remove(args) => run_remove(&args.name, store),
    }
}

async fn run_add(args: AddArgs, store: &mut Store) -> anyhow::Result<Value> {
    let (host, port, user, password) = if let Some(mysql_uri) = &args.mysql {
        parse_mysql_uri(mysql_uri)?
    } else if let Some(host) = &args.host {
        (
            host.clone(),
            args.port,
            args.user.clone().unwrap_or_else(|| "root".to_string()),
            args.password.clone().unwrap_or_default(),
        )
    } else {
        anyhow::bail!(
            "Specify --mysql <uri> or --host <host> --user <user> --password <pass>."
        );
    };

    let config = EnvironmentConfig {
        host: host.clone(),
        mysql_port: port,
        http_port: args.http_port,
        user: user.clone(),
    };

    let creds = EnvironmentCredentials {
        password: password.clone(),
    };

    store.add_env(&args.name, config, creds)?;

    // Probe HTTP to surface port/connectivity issues now, not later inside
    // `profile get`. The store already saved, so probe failures are warnings.
    let env = store.resolve_env(&args.name)?;
    let probe = HttpConnection::new(&env).probe().await;

    let mut result = json!({
        "status": "added",
        "name": args.name,
        "host": host,
        "mysql_port": port,
        "http_port": args.http_port,
        "user": user,
        "http_probe": probe_to_json(&probe),
    });

    if !probe.any_ok() {
        // Configured port doesn't respond — try common alternatives before giving up.
        let mut suggestions: Vec<Value> = Vec::new();
        for alt in common_http_ports_except(args.http_port) {
            let alt_probe = HttpConnection::for_target(&env, &host, alt).probe().await;
            if alt_probe.any_ok() {
                suggestions.push(json!({
                    "http_port": alt,
                    "rest_v2": alt_probe.rest_v2.status.short(),
                    "legacy": alt_probe.legacy.status.short(),
                    "fix": format!(
                        "{bin} auth remove {name} && {bin} auth add {name} --host {host} --user {user} --password '<pass>' --http-port {alt}",
                        bin = store.product.binary,
                        name = args.name,
                        host = host,
                        user = user,
                    ),
                }));
            }
        }

        let mut warnings: Vec<String> = vec![format!(
            "HTTP probe failed on port {}. REST v2: {}; legacy: {}. Profile commands will not work until this is fixed.",
            args.http_port,
            probe.rest_v2.status.short(),
            probe.legacy.status.short(),
        )];
        if !suggestions.is_empty() {
            warnings.push("Another port responded — see http_port_suggestions below.".to_string());
            result["http_port_suggestions"] = Value::Array(suggestions);
        }
        result["warnings"] = json!(warnings);
    }

    Ok(result)
}

fn common_http_ports_except(configured: u16) -> Vec<u16> {
    // 8080: Cloud FE HTTP (routed through Stream Load port).
    // 8030: Apache Doris / self-hosted default.
    [8080, 8030, 8040]
        .into_iter()
        .filter(|p| *p != configured)
        .collect()
}

fn probe_to_json(p: &HttpProbe) -> Value {
    json!({
        "rest_v2": {
            "url": p.rest_v2.url,
            "status": p.rest_v2.status.short(),
            "ok": matches!(p.rest_v2.status, ProbeStatus::Ok),
        },
        "legacy": {
            "url": p.legacy.url,
            "status": p.legacy.status.short(),
            "ok": matches!(p.legacy.status, ProbeStatus::Ok),
        },
    })
}

fn run_list(store: &Store) -> anyhow::Result<Value> {
    let envs = store.list_envs();
    let list: Vec<Value> = envs
        .iter()
        .map(|(name, config, is_default)| {
            json!({
                "name": name,
                "host": config.host,
                "port": config.mysql_port,
                "user": config.user,
                "type": "self_hosted",
                "default": is_default,
            })
        })
        .collect();

    if list.is_empty() {
        Ok(json!({
            "environments": [],
            "message": format!(
                "No environments configured. Run `{} auth add <name> --host <host> --user <user> --password <pass>` to get started.",
                store.product.binary
            )
        }))
    } else {
        Ok(Value::Array(list))
    }
}

async fn run_status(store: &Store, env_flag: &str) -> anyhow::Result<Value> {
    let env_name = store.effective_env_name(env_flag);
    let (source, source_detail) = store.effective_env_source(env_flag);
    let env = store.resolve_env(&env_name)?;

    let all_envs: Vec<String> = store
        .list_envs()
        .iter()
        .map(|(n, _, _)| n.clone())
        .collect();

    let mut result = json!({
        "environment": env_name,
        "source": source,
        "source_detail": source_detail,
        "all_environments": all_envs,
        "type": "self_hosted",
        "host": env.host,
        "mysql_port": env.mysql_port,
        "http_port": env.http_port,
        "user": env.user,
    });

    // Test MySQL connection + gather cluster info
    match MysqlConnection::connect(&env).await {
        Ok(mut conn) => match conn.ping().await {
            Ok(latency) => {
                result["mysql_status"] = json!("connected");
                result["mysql_latency_ms"] = json!(latency);

                if let Ok(ver) = conn.query("SELECT VERSION() as v").await {
                    if let Some(v) = ver.rows.first().and_then(|r| r.get("v")) {
                        result["doris_version"] = v.clone();
                    }
                }

                if let Ok(bes) = conn.query("SHOW BACKENDS").await {
                    let backends: Vec<Value> = bes.rows.iter().map(|row| {
                        json!({
                            "id": row.get("BackendId").or_else(|| row.get("backend_id")),
                            "host": row.get("Host").or_else(|| row.get("IP")).and_then(|v| v.as_str()),
                            "alive": row.get("Alive").and_then(|v| v.as_str()).map(|s| s == "true"),
                        })
                    }).collect();
                    result["backends"] = json!(backends);
                }

                if let Ok(wgs) = conn.query("SHOW WORKLOAD GROUPS").await {
                    let groups: Vec<Value> = wgs
                        .rows
                        .iter()
                        .map(|row| {
                            json!({
                                "name": row.get("Name").and_then(|v| v.as_str()),
                                "running": row.get("running_query_num"),
                                "waiting": row.get("waiting_query_num"),
                            })
                        })
                        .collect();
                    result["workload_groups"] = json!(groups);
                }

                if let Ok(vars) = conn.query("SELECT COUNT(*) as cnt FROM information_schema.session_variables WHERE CHANGED = 1").await {
                    if let Some(cnt) = vars.rows.first().and_then(|r| r.get("cnt")) {
                        result["changed_session_vars"] = cnt.clone();
                    }
                }
            }
            Err(e) => {
                result["mysql_status"] = json!(format!("error: {e}"));
            }
        },
        Err(e) => {
            result["mysql_status"] = json!(format!("error: {e}"));
        }
    }

    // HTTP health — profile features need this.
    let probe = HttpConnection::new(&env).probe().await;
    result["http_status"] = if probe.any_ok() {
        json!("connected")
    } else {
        json!("unreachable")
    };
    result["http_probe"] = probe_to_json(&probe);

    Ok(result)
}

fn run_remove(name: &str, store: &mut Store) -> anyhow::Result<Value> {
    store.remove_env(name)?;
    Ok(json!({
        "status": "removed",
        "name": name,
    }))
}

/// Parse a MySQL URI like mysql://user:pass@host:port/db
fn parse_mysql_uri(uri: &str) -> anyhow::Result<(String, u16, String, String)> {
    let uri = uri
        .strip_prefix("mysql://")
        .ok_or_else(|| anyhow::anyhow!("MySQL URI must start with mysql://"))?;

    let (userinfo, hostport) = if let Some(at_pos) = uri.rfind('@') {
        (&uri[..at_pos], &uri[at_pos + 1..])
    } else {
        ("root:", uri)
    };

    // Strip database path if present
    let hostport = hostport.split('/').next().unwrap_or(hostport);

    let (user, password) = if let Some(colon) = userinfo.find(':') {
        (&userinfo[..colon], &userinfo[colon + 1..])
    } else {
        (userinfo, "")
    };

    let (host, port) = if let Some(colon) = hostport.rfind(':') {
        let port_str = &hostport[colon + 1..];
        let port: u16 = port_str
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid port: {port_str}"))?;
        (&hostport[..colon], port)
    } else {
        (hostport, 9030u16)
    };

    Ok((
        host.to_string(),
        port,
        user.to_string(),
        password.to_string(),
    ))
}
