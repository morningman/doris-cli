use crate::models::profile::*;
use crate::parser::{fragment_parser, section_parser, value_parser};

/// Parse a raw Doris profile text into a structured DorisProfile.
pub fn parse(raw_text: &str) -> DorisProfile {
    let normalized = section_parser::normalize_text(raw_text);
    let sections = section_parser::split_sections(&normalized);

    // Parse Summary
    let summary_text = sections.get("Summary").map(|s| s.as_str()).unwrap_or("");
    let summary_kv = section_parser::parse_kv_section(summary_text);
    let total_time_str = summary_kv.get("Total").cloned().unwrap_or_default();
    let total_time_ms = value_parser::parse_duration_ms(&total_time_str);

    let summary = ProfileSummary {
        query_id: summary_kv.get("Profile ID").cloned().unwrap_or_default(),
        total_time: total_time_str,
        total_time_ms,
        state: summary_kv.get("Task State").cloned().unwrap_or_default(),
        sql: summary_kv.get("Sql Statement").cloned().unwrap_or_default(),
        doris_version: summary_kv.get("Doris Version").cloned(),
        is_nereids: summary_kv.get("Is Nereids").map(|v| v == "Yes"),
        is_cached: summary_kv.get("Is Cached").map(|v| v == "Yes"),
        total_instances: summary_kv
            .get("Total Instances Num")
            .and_then(|v| v.parse().ok()),
        user: summary_kv.get("User").cloned(),
        start_time: summary_kv.get("Start Time").cloned(),
        end_time: summary_kv.get("End Time").cloned(),
        default_db: summary_kv.get("Default Db").cloned(),
    };

    // Parse Execution Summary
    let exec_text = sections
        .get("Execution Summary")
        .map(|s| s.as_str())
        .unwrap_or("");
    let exec_kv = section_parser::parse_kv_section(exec_text);

    let execution_summary = ExecutionSummary {
        parse_sql_time: exec_kv.get("Parse SQL Time").cloned(),
        plan_time: exec_kv.get("Plan Time").cloned(),
        schedule_time: exec_kv.get("Schedule Time").cloned(),
        wait_fetch_result_time: exec_kv.get("Wait and Fetch Result Time").cloned(),
        fetch_result_time: exec_kv.get("Fetch Result Time").cloned(),
        write_result_time: exec_kv.get("Write Result Time").cloned(),
        nereids_analysis_time: exec_kv.get("Nereids Analysis Time").cloned(),
        nereids_rewrite_time: exec_kv.get("Nereids Rewrite Time").cloned(),
        nereids_optimize_time: exec_kv.get("Nereids Optimize Time").cloned(),
        workload_group: exec_kv.get("Workload Group").cloned(),
    };

    // Parse Changed Session Variables
    let vars_text = sections
        .get("Changed Session Variables")
        .map(|s| s.as_str())
        .unwrap_or("");
    let changed_session_vars = parse_session_vars(vars_text);

    // Parse MergedProfile
    let merged_text = sections
        .get("MergedProfile")
        .map(|s| s.as_str())
        .unwrap_or("");
    let fragments = fragment_parser::parse_merged_profile(merged_text);

    DorisProfile {
        summary,
        execution_summary,
        changed_session_vars,
        fragments,
    }
}

fn parse_session_vars(text: &str) -> Vec<SessionVar> {
    let mut vars = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        // Skip header and separator lines
        if line.is_empty()
            || line.starts_with("VarName")
            || line.starts_with("---")
            || line.starts_with('-')
        {
            continue;
        }

        let parts: Vec<&str> = line.split('|').map(|s| s.trim()).collect();
        if parts.len() >= 3 {
            vars.push(SessionVar {
                name: parts[0].to_string(),
                current_value: parts[1].to_string(),
                default_value: parts[2].to_string(),
            });
        }
    }
    vars
}

/// Extract all operators from a parsed profile, flattened.
pub fn flatten_operators(profile: &DorisProfile) -> Vec<FlatOperator> {
    let total_time_ms = profile.summary.total_time_ms.unwrap_or(1.0).max(0.001);
    let mut result = Vec::new();

    for fragment in &profile.fragments {
        for pipeline in &fragment.pipelines {
            for op in &pipeline.operators {
                let exec_time_avg_ms = op
                    .metrics
                    .exec_time
                    .as_ref()
                    .and_then(|v| v.avg)
                    .unwrap_or(0.0);

                let input_rows = op.metrics.input_rows.as_ref().and_then(|v| v.sum.or(v.avg));

                let output_rows = op
                    .metrics
                    .rows_produced
                    .as_ref()
                    .and_then(|v| v.sum.or(v.avg));

                let skew_ratio = op.metrics.exec_time.as_ref().and_then(|v| v.skew_ratio());

                let time_pct = (exec_time_avg_ms / total_time_ms) * 100.0;

                // Selectivity: input / output ratio
                let selectivity = match (input_rows, output_rows) {
                    (Some(inp), Some(out)) if out > 0.0 => Some(round2(inp / out)),
                    _ => None,
                };

                // Spill detection
                let spill_write = op
                    .all_counters
                    .get("SpillWriteBytes")
                    .or_else(|| op.all_counters.get("SpillWriteBytesToLocalStorage"))
                    .and_then(|v| v.sum.or(v.avg))
                    .unwrap_or(0.0);
                let spilled = if spill_write > 0.0 { Some(true) } else { None };

                // Peak memory
                let peak_mem_bytes = op
                    .metrics
                    .memory_usage_peak
                    .as_ref()
                    .and_then(|v| v.sum.or(v.avg))
                    .filter(|&v| v > 0.0);

                // Wait dependency analysis: blocked if wait_time > exec_time * 2
                let wait_time_ms = op
                    .metrics
                    .wait_for_dependency_time
                    .as_ref()
                    .and_then(|v| v.avg)
                    .filter(|&v| v > 0.0);

                let blocked_on_upstream = match (wait_time_ms, exec_time_avg_ms) {
                    (Some(wait), exec) if exec > 0.0 && wait > exec * 2.0 => Some(true),
                    _ => None,
                };

                // Runtime filter extraction from plan_info AND all_counters
                let mut runtime_filters: Vec<String> = Vec::new();

                // From plan_info: keys containing RF references
                for (k, v) in &op.plan_info {
                    if k.contains("RF") || k.contains("runtime") || k.contains("RuntimeFilter") {
                        runtime_filters.push(format!("{k}: {v}"));
                    }
                }

                // From all_counters: RuntimeFilter-related counters
                for (k, v) in &op.all_counters {
                    if k.contains("RuntimeFilter") || k.starts_with("RF") {
                        let val = v.avg.or(v.sum).unwrap_or(0.0);
                        if val > 0.0 {
                            runtime_filters.push(format!("{k}: {val}"));
                        }
                    }
                }

                // From all_counters: check for runtime filter specific metrics
                let rf_input = op
                    .all_counters
                    .get("RuntimeFilterInput")
                    .or_else(|| op.all_counters.get("RuntimeFilterNum"))
                    .and_then(|v| v.sum.or(v.avg));
                if let Some(rf_count) = rf_input {
                    if rf_count > 0.0
                        && !runtime_filters.iter().any(|r| r.contains("RuntimeFilter"))
                    {
                        runtime_filters.push(format!("RuntimeFiltersApplied: {rf_count}"));
                    }
                }

                let runtime_filters = if runtime_filters.is_empty() {
                    None
                } else {
                    Some(runtime_filters)
                };

                // Shuffle bytes (for exchange/data_stream operators)
                let shuffle_bytes = op
                    .all_counters
                    .get("ShuffleSendBytes")
                    .or_else(|| op.all_counters.get("BytesSent"))
                    .or_else(|| op.all_counters.get("OverallThroughput"))
                    .and_then(|v| v.sum.or(v.avg))
                    .filter(|&v| v > 0.0);

                // Cache hit percentage (for scan operators)
                let cache_hit_pct = {
                    let hit = op
                        .all_counters
                        .get("FileCacheHitBytes")
                        .or_else(|| op.all_counters.get("CacheHitBytes"))
                        .and_then(|v| v.sum.or(v.avg))
                        .unwrap_or(0.0);
                    let miss = op
                        .all_counters
                        .get("FileCacheMissBytes")
                        .or_else(|| op.all_counters.get("CacheMissBytes"))
                        .and_then(|v| v.sum.or(v.avg))
                        .unwrap_or(0.0);
                    let total = hit + miss;
                    if total > 0.0 {
                        Some(round2((hit / total) * 100.0))
                    } else {
                        None
                    }
                };

                // Rows filtered (scan: output < input means filtering happened)
                let rows_filtered = match (input_rows, output_rows) {
                    (Some(inp), Some(out)) if inp > out && inp > 0.0 => Some(inp - out),
                    _ => None,
                };

                // Join type from PlanInfo
                let join_type = op
                    .plan_info
                    .iter()
                    .find_map(|(k, v)| {
                        let combined = format!("{k} {v}").to_lowercase();
                        if combined.contains("broadcast") {
                            Some("broadcast".to_string())
                        } else if combined.contains("bucket_shuffle")
                            || combined.contains("bucketshuffle")
                        {
                            Some("bucket_shuffle".to_string())
                        } else if combined.contains("colocate") {
                            Some("colocated".to_string())
                        } else if combined.contains("shuffle") || combined.contains("hash") {
                            Some("shuffle".to_string())
                        } else {
                            None
                        }
                    })
                    // Also try extracting from operator name
                    .or_else(|| {
                        let name_lower = op.info.full_name.to_lowercase();
                        if name_lower.contains("broadcast") {
                            Some("broadcast".to_string())
                        } else if name_lower.contains("colocate") {
                            Some("colocated".to_string())
                        } else {
                            None
                        }
                    });

                result.push(FlatOperator {
                    name: op.info.full_name.clone(),
                    table: op.info.table_name.clone(),
                    frag: fragment.id,
                    pipeline: pipeline.id,
                    exec_time_avg_ms: round2(exec_time_avg_ms),
                    input_rows,
                    output_rows,
                    skew_ratio: skew_ratio.map(round2),
                    time_pct: round2(time_pct),
                    selectivity,
                    spilled,
                    peak_mem_bytes,
                    blocked_on_upstream,
                    wait_time_ms: wait_time_ms.map(round2),
                    runtime_filters,
                    shuffle_bytes,
                    cache_hit_pct,
                    rows_filtered,
                    join_type,
                });
            }
        }
    }

    // Sort by exec_time descending
    result.sort_by(|a, b| {
        b.exec_time_avg_ms
            .partial_cmp(&a.exec_time_avg_ms)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    result
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}
