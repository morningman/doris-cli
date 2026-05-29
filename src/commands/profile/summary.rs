use crate::config::Environment;
use crate::connection::MysqlConnection;
use crate::parser::profile_parser;
use crate::parser::section_parser;
use crate::parser::value_parser;
use serde_json::{json, Value};

/// Default profile output: summary + plan + top operators + fragment breakdown + scanned table DDLs.
/// ONE call = everything the agent needs for slow query diagnosis.
pub async fn run(
    query_id: &str,
    profile_text: Option<&str>,
    env: &Environment,
) -> anyhow::Result<Value> {
    if let Some(text) = profile_text {
        return run_full(text, env, None, None, Vec::new()).await;
    }

    // Track the fetch outcome so we can surface *why* when the fallback fires.
    let fetch_failure = match super::fetch::fetch_profile_text(query_id, env).await {
        Ok(fetched) => {
            let attempts = fetched.attempts.clone();
            return run_full(
                &fetched.text,
                env,
                Some(fetched.served_by),
                Some(fetched.via),
                attempts,
            )
            .await;
        }
        Err(f) => f,
    };

    // Fallback: SQL summary (no profile text available anywhere).
    if let Some(summary) = super::fetch::fetch_summary_from_sql(query_id, env).await? {
        let total_time_str = summary
            .get("total_time")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let total_time_ms = value_parser::parse_duration_ms(total_time_str);

        let note = build_fallback_note(&fetch_failure);

        return Ok(json!({
            "summary": {
                "query_id": summary.get("query_id"),
                "total_time": total_time_str,
                "total_time_ms": total_time_ms,
                "state": summary.get("state"),
                "sql": summary.get("sql"),
            },
            "time_breakdown": null,
            "operators": [],
            "note": note,
            "fetch_attempts": fetch_failure.attempts_json(),
        }));
    }

    // Not even in SHOW QUERY PROFILE — bail with the full attempt log.
    anyhow::bail!("{fetch_failure}")
}

/// Human-readable note for the fallback path.
/// Picks the most specific hint we can infer from the attempts we saw.
fn build_fallback_note(failure: &super::fetch::FetchFailure) -> String {
    let attempts = &failure.attempts;

    let any_unreachable = attempts
        .iter()
        .any(|a| a.result.starts_with("Connection failed"));
    if any_unreachable {
        return format!(
            "HTTP endpoint unreachable on configured port. Cloud deployments use \
             --http-port 8080; self-hosted Doris uses 8030. Run `{} auth status` \
             to verify, or re-add auth with the right port.",
            failure.binary
        );
    }

    let all_4xx_5xx = !attempts.is_empty()
        && attempts.iter().all(|a| {
            a.result.starts_with("HTTP error [4") || a.result.starts_with("HTTP error [5")
        });
    if all_4xx_5xx {
        return "Profile not found. REST v2 with is_all_node=true already aggregates \
                across FEs, so this usually means the profile was evicted (Doris keeps \
                only a limited number per FE). Try immediately after the query runs, \
                or raise max_query_profile_num on the FEs."
            .to_string();
    }

    "Full profile text not accessible. See fetch_attempts for details, or pass \
     --file <profile.txt> exported from the web UI."
        .to_string()
}

/// Build the complete profile output with all context.
async fn run_full(
    text: &str,
    env: &Environment,
    served_by: Option<String>,
    via: Option<&'static str>,
    attempts: Vec<super::fetch::FetchAttempt>,
) -> anyhow::Result<Value> {
    let profile = profile_parser::parse(text);
    let flat_ops = profile_parser::flatten_operators(&profile);

    let top_ops: Vec<Value> = flat_ops
        .iter()
        .map(|op| serde_json::to_value(op).unwrap_or(Value::Null))
        .collect();

    // Physical plan
    let normalized = section_parser::normalize_text(text);
    let sections = section_parser::split_sections(&normalized);
    let physical_plan = sections.get("Physical Plan").cloned();

    // Session vars with impact classification
    let session_vars: Vec<Value> = profile
        .changed_session_vars
        .iter()
        .map(|var| {
            json!({
                "name": var.name,
                "value": var.current_value,
                "default": var.default_value,
                "impact": classify_session_var_impact(&var.name),
            })
        })
        .collect();

    // Fragment-level breakdown: aggregate operator times per fragment
    let fragments: Vec<Value> = profile
        .fragments
        .iter()
        .map(|frag| {
            let mut total_exec_ms: f64 = 0.0;
            let mut total_shuffle_bytes: f64 = 0.0;
            let mut instance_num: i32 = 0;

            for pipeline in &frag.pipelines {
                if pipeline.instance_num > instance_num {
                    instance_num = pipeline.instance_num;
                }
                for op in &pipeline.operators {
                    total_exec_ms += op
                        .metrics
                        .exec_time
                        .as_ref()
                        .and_then(|v| v.avg)
                        .unwrap_or(0.0);

                    // Accumulate shuffle bytes from exchange operators
                    total_shuffle_bytes += op
                        .all_counters
                        .get("ShuffleSendBytes")
                        .or_else(|| op.all_counters.get("BytesSent"))
                        .and_then(|v| v.sum.or(v.avg))
                        .unwrap_or(0.0);
                }
            }

            let mut frag_json = json!({
                "id": frag.id,
                "exec_time_ms": round2(total_exec_ms),
                "instances": instance_num,
                "pipelines": frag.pipelines.len(),
            });

            if total_shuffle_bytes > 0.0 {
                frag_json["shuffle_bytes"] = json!(total_shuffle_bytes as u64);
            }

            frag_json
        })
        .collect();

    // Extract scanned table names from operators
    let scanned_tables: Vec<String> = flat_ops
        .iter()
        .filter_map(|op| op.table.as_ref())
        .map(|t| t.split('(').next().unwrap_or(t).trim().to_string())
        .filter(|t| !t.is_empty())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    // Auto-fetch DDL + health for each scanned table (the key merge)
    let mut table_context: serde_json::Map<String, Value> = serde_json::Map::new();
    if !scanned_tables.is_empty() {
        if let Ok(mut conn) = MysqlConnection::connect(env).await {
            // Try to USE the database from profile or from table names
            let db_to_use = profile
                .summary
                .default_db
                .as_deref()
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    scanned_tables
                        .iter()
                        .find(|t| t.contains('.'))
                        .and_then(|t| t.split('.').next())
                });
            if let Some(db) = db_to_use {
                let _ = conn.exec(&format!("USE `{db}`")).await;
            }

            for table in &scanned_tables {
                let mut entry = json!({});

                // Handle db.table format: try full name first, then just table name
                let ddl_queries = if table.contains('.') {
                    let parts: Vec<&str> = table.split('.').collect();
                    vec![
                        format!("SHOW CREATE TABLE `{}`.`{}`", parts[0], parts[1]),
                        format!("SHOW CREATE TABLE `{}`", parts.last().unwrap_or(&"")),
                    ]
                } else {
                    vec![format!("SHOW CREATE TABLE `{table}`")]
                };

                // DDL
                for ddl_sql in &ddl_queries {
                    if let Ok(ddl_result) = conn.query(ddl_sql).await {
                        if let Some(ddl) = ddl_result
                            .rows
                            .first()
                            .and_then(|r| r.get("Create Table"))
                            .and_then(|v| v.as_str())
                        {
                            entry["ddl"] = Value::String(ddl.to_string());
                            break;
                        }
                    }
                }

                // Tablet health from SHOW DATA SKEW
                let tbl_name = table.split('.').last().unwrap_or(table);
                if let Ok(skew_result) = conn
                    .query(&format!("SHOW DATA SKEW FROM `{tbl_name}`"))
                    .await
                {
                    let mut sizes: Vec<f64> = Vec::new();
                    let mut total_rows: u64 = 0;
                    for row in &skew_result.rows {
                        let size = row
                            .get("AvgDataSize")
                            .and_then(|v| {
                                v.as_u64()
                                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                            })
                            .unwrap_or(0) as f64;
                        sizes.push(size);
                        total_rows += row
                            .get("AvgRowCount")
                            .and_then(|v| {
                                v.as_u64()
                                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                            })
                            .unwrap_or(0);
                    }

                    if !sizes.is_empty() {
                        let avg = sizes.iter().sum::<f64>() / sizes.len() as f64;
                        let max = sizes.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                        let skew = if avg > 0.0 { max / avg } else { 1.0 };
                        let total_bytes: f64 = sizes.iter().sum();

                        entry["total_rows"] = json!(total_rows);
                        entry["total_size_gb"] =
                            json!(round2(total_bytes / (1024.0 * 1024.0 * 1024.0)));
                        entry["tablets"] = json!(sizes.len());
                        entry["tablet_skew"] = json!(round1(skew));
                    }
                }

                table_context.insert(table.clone(), entry);
            }
        }
    }

    // Query-level aggregate stats (the "is this bad?" signal)
    let total_scan_rows: f64 = flat_ops
        .iter()
        .filter(|op| op.name.contains("SCAN"))
        .filter_map(|op| op.output_rows)
        .sum();
    let total_shuffle_bytes: f64 = flat_ops.iter().filter_map(|op| op.shuffle_bytes).sum();
    let total_peak_mem: f64 = flat_ops.iter().filter_map(|op| op.peak_mem_bytes).sum();
    let spilled_count = flat_ops
        .iter()
        .filter(|op| op.spilled == Some(true))
        .count();
    let blocked_count = flat_ops
        .iter()
        .filter(|op| op.blocked_on_upstream == Some(true))
        .count();

    let mut result = json!({
        "summary": {
            "query_id": profile.summary.query_id,
            "total_time": profile.summary.total_time,
            "total_time_ms": profile.summary.total_time_ms,
            "state": profile.summary.state,
            "sql": profile.summary.sql,
            "doris_version": profile.summary.doris_version,
            "is_nereids": profile.summary.is_nereids,
            "instances": profile.summary.total_instances,
            "workload_group": profile.execution_summary.workload_group,
        },
        "time_breakdown": {
            "parse_sql": profile.execution_summary.parse_sql_time,
            "plan": profile.execution_summary.plan_time,
            "schedule": profile.execution_summary.schedule_time,
            "wait_fetch_result": profile.execution_summary.wait_fetch_result_time,
            "fetch_result": profile.execution_summary.fetch_result_time,
            "nereids_analysis": profile.execution_summary.nereids_analysis_time,
            "nereids_rewrite": profile.execution_summary.nereids_rewrite_time,
            "nereids_optimize": profile.execution_summary.nereids_optimize_time,
        },
        "query_stats": {
            "total_scan_rows": if total_scan_rows > 0.0 { json!(total_scan_rows as u64) } else { json!(null) },
            "total_shuffle_bytes": if total_shuffle_bytes > 0.0 { json!(total_shuffle_bytes as u64) } else { json!(null) },
            "total_peak_memory_bytes": if total_peak_mem > 0.0 { json!(total_peak_mem as u64) } else { json!(null) },
            "spilled_operators": spilled_count,
            "blocked_operators": blocked_count,
            "operator_count": flat_ops.len(),
            "fragment_count": profile.fragments.len(),
        },
        "fragments": fragments,
        "operators": top_ops,
    });

    if let Some(plan) = physical_plan {
        result["physical_plan"] = Value::String(plan);
    }
    if !session_vars.is_empty() {
        result["changed_session_vars"] = Value::Array(session_vars);
    }
    if !table_context.is_empty() {
        result["scanned_tables"] = Value::Object(table_context);
    }

    // Provenance for multi-FE clusters: tell the caller which FE served this
    // and, if we had to fall back, what we tried.
    if let Some(sb) = served_by {
        result["served_by"] = Value::String(sb);
    }
    if let Some(v) = via {
        result["fetch_via"] = Value::String(v.to_string());
    }
    if !attempts.is_empty() {
        let attempts_json: Vec<Value> = attempts.iter().map(|a| a.to_json()).collect();
        result["fetch_attempts"] = Value::Array(attempts_json);
    }

    Ok(result)
}

/// Public accessor for session variable impact classification.
pub fn classify_var_impact(name: &str) -> &'static str {
    classify_session_var_impact(name)
}

/// Classify session variable by performance impact category.
/// Based on actual Doris SessionVariable.java source code categories.
fn classify_session_var_impact(name: &str) -> &'static str {
    // Exact matches first (from Doris SessionVariable.java)
    match name {
        // Memory: exec limits, spill thresholds, buffer sizes
        "exec_mem_limit"
        | "scan_queue_mem_limit"
        | "local_exchange_free_blocks_limit"
        | "spill_min_revocable_mem"
        | "spill_sort_mem_limit"
        | "spill_streaming_agg_mem_limit"
        | "full_sort_max_buffered_bytes"
        | "minimum_operator_memory_required_kb"
        | "exchange_multi_blocks_byte_size"
        | "enable_spill"
        | "enable_force_spill"
        | "spill_sort_batch_bytes"
        | "spill_aggregation_partition_count"
        | "spill_hash_join_partition_count"
        | "spill_revocable_memory_high_watermark_percent"
        | "enable_reserve_memory"
        | "low_memory_mode_buffer_limit"
        | "data_queue_max_blocks" => "memory",

        // Parallelism: instance counts, scanner concurrency, pipeline tasks
        "parallel_fragment_exec_instance_num"
        | "parallel_pipeline_task_num"
        | "max_scanners_concurrency"
        | "max_file_scanners_concurrency"
        | "min_scanners_concurrency"
        | "min_file_scanners_concurrency"
        | "min_scan_scheduler_concurrency"
        | "colocate_max_parallel_num"
        | "parallel_exchange_instance_num"
        | "parallel_scan_max_scanners_count"
        | "parallel_scan_min_rows_per_scanner"
        | "max_instance_num"
        | "max_column_reader_num"
        | "send_batch_parallelism"
        | "query_slot_count" => "parallelism",

        // Runtime filter: filter types, sizes, wait times
        "runtime_filter_mode"
        | "runtime_filter_type"
        | "runtime_filter_wait_time_ms"
        | "runtime_filter_wait_infinitely"
        | "runtime_filters_max_num"
        | "runtime_filter_max_in_num"
        | "runtime_bloom_filter_size"
        | "runtime_bloom_filter_min_size"
        | "runtime_bloom_filter_max_size"
        | "enable_runtime_filter_prune"
        | "enable_sync_runtime_filter_size"
        | "expand_runtime_filter_by_inner_join" => "filter",

        // Join/aggregation: join methods, reorder, broadcast thresholds
        "broadcast_row_count_limit"
        | "broadcast_hashtable_mem_limit_percentage"
        | "auto_broadcast_join_threshold"
        | "disable_colocate_plan"
        | "enable_bucket_shuffle_join"
        | "prefer_join_method"
        | "disable_join_reorder"
        | "enable_cost_based_join_reorder"
        | "max_join_number_of_reorder"
        | "enable_bushy_tree"
        | "disable_streaming_preaggregations"
        | "enable_distinct_streaming_aggregation"
        | "batch_size"
        | "enable_push_down_no_group_agg" => "optimization",

        // Scan/IO: scan modes, pushdown, file format options
        "enable_parallel_scan"
        | "enable_shared_scan"
        | "enable_local_shuffle"
        | "force_to_local_shuffle"
        | "enable_local_merge_sort"
        | "enable_orc_lazy_materialization"
        | "enable_parquet_lazy_materialization"
        | "enable_parquet_filter_by_bloom_filter"
        | "enable_parquet_filter_by_min_max"
        | "file_split_size"
        | "max_initial_file_split_size"
        | "max_scan_key_num"
        | "max_pushdown_conditions_per_column"
        | "enable_common_expr_pushdown"
        | "enable_count_on_index_pushdown"
        | "partition_pruning_expand_threshold"
        | "enable_projection" => "io",

        // Cache: SQL cache, file cache, query cache, page cache
        "enable_sql_cache"
        | "enable_query_cache"
        | "query_cache_force_refresh"
        | "query_cache_entry_max_bytes"
        | "query_cache_entry_max_rows"
        | "enable_file_cache"
        | "disable_file_cache"
        | "file_cache_query_limit_percent"
        | "enable_page_cache"
        | "enable_segment_cache"
        | "enable_inverted_index_query_cache"
        | "enable_inverted_index_searcher_cache" => "cache",

        // Timeout: query, insert, connection timeouts
        "query_timeout"
        | "insert_timeout"
        | "analyze_timeout"
        | "max_execution_time"
        | "insert_visible_timeout_ms"
        | "interactive_timeout"
        | "wait_timeout"
        | "net_write_timeout"
        | "net_read_timeout" => "timeout",

        // Optimization: planner settings, predicate handling
        "enable_nereids_planner"
        | "enable_partition_topn"
        | "enable_infer_predicate"
        | "enable_short_circuit_query"
        | "enable_fold_constant_by_be"
        | "skip_prune_predicate"
        | "topn_opt_limit_threshold"
        | "enable_two_phase_read_opt"
        | "enable_stats"
        | "experimental_enable_agg_state" => "optimization",

        // Fallback: pattern-based for variables not in the exact list
        _ => classify_by_pattern(name),
    }
}

fn classify_by_pattern(name: &str) -> &'static str {
    let n = name.to_lowercase();
    if n.contains("mem") || n.contains("spill") || n.contains("buffer") {
        "memory"
    } else if n.contains("parallel") || n.contains("instance") || n.contains("thread") {
        "parallelism"
    } else if n.contains("runtime_filter") || n.contains("bloom_filter") {
        "filter"
    } else if n.contains("cache") {
        "cache"
    } else if n.contains("timeout") {
        "timeout"
    } else if n.contains("scan") || n.contains("pushdown") {
        "io"
    } else if n.contains("join")
        || n.contains("reorder")
        || n.contains("nereids")
        || n.contains("optimize")
    {
        "optimization"
    } else {
        "other"
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}
