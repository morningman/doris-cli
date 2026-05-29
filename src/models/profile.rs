use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── Aggregate Value (Doris "sum X, avg Y, max Z, min W") ───

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AggValue {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sum: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

impl AggValue {
    /// The best representative value.
    #[allow(dead_code)]
    pub fn representative(&self) -> f64 {
        self.avg.or(self.sum).unwrap_or(0.0)
    }

    /// Skew ratio: max / avg.
    pub fn skew_ratio(&self) -> Option<f64> {
        match (self.max, self.avg) {
            (Some(max), Some(avg)) if avg > 0.0 => Some(max / avg),
            _ => None,
        }
    }
}

// ─── Operator Info ───

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorInfoModel {
    pub full_name: String,
    pub operator_type: String,
    pub id: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nereids_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dest_id: Option<i32>,
    pub is_sink: bool,
}

// ─── Operator Metrics ───

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OperatorMetrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exec_time: Option<AggValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_rows: Option<AggValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows_produced: Option<AggValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_usage: Option<AggValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_usage_peak: Option<AggValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_time: Option<AggValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub close_time: Option<AggValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub init_time: Option<AggValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait_for_dependency_time: Option<AggValue>,
}

// ─── Operator ───

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operator {
    pub info: OperatorInfoModel,
    pub metrics: OperatorMetrics,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub all_counters: HashMap<String, AggValue>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub plan_info: HashMap<String, String>,
}

// ─── Pipeline ───

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pipeline {
    pub id: i32,
    pub instance_num: i32,
    pub operators: Vec<Operator>,
}

// ─── Fragment ───

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fragment {
    pub id: i32,
    pub pipelines: Vec<Pipeline>,
}

// ─── Profile Summary ───

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfileSummary {
    pub query_id: String,
    pub total_time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_time_ms: Option<f64>,
    pub state: String,
    pub sql: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doris_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_nereids: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_cached: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_instances: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_db: Option<String>,
}

// ─── Execution Summary ───

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecutionSummary {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_sql_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait_fetch_result_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetch_result_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write_result_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nereids_analysis_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nereids_rewrite_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nereids_optimize_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workload_group: Option<String>,
}

// ─── Session Variable ───

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionVar {
    pub name: String,
    pub current_value: String,
    pub default_value: String,
}

// ─── Full Profile ───

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DorisProfile {
    pub summary: ProfileSummary,
    pub execution_summary: ExecutionSummary,
    pub changed_session_vars: Vec<SessionVar>,
    pub fragments: Vec<Fragment>,
}

// ─── Flat Operator (for Level 0/1 output) ───

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlatOperator {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    pub frag: i32,
    pub pipeline: i32,
    pub exec_time_avg_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_rows: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_rows: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skew_ratio: Option<f64>,
    pub time_pct: f64,

    // ─── Diagnostic fields (user feedback items 3-6) ───
    /// Selectivity: input_rows / output_rows. High ratio = bad key/filter.
    /// >1000 on scan = wrong sort key. >100 on join = missing runtime filter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selectivity: Option<f64>,

    /// True if this operator spilled to disk (SpillWriteBytes > 0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spilled: Option<bool>,

    /// Peak memory in bytes for this operator.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_mem_bytes: Option<f64>,

    /// True if wait_time > exec_time * 2 — bottleneck is upstream, not here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_on_upstream: Option<bool>,

    /// Wait-for-dependency time in ms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait_time_ms: Option<f64>,

    /// Runtime filter info extracted from PlanInfo (e.g., "RF0[col->col](ndv/size)")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_filters: Option<Vec<String>>,

    // ─── Infrastructure metrics (surfaced from all_counters) ───
    /// Network shuffle bytes sent by this operator (exchange/data_stream).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shuffle_bytes: Option<f64>,

    /// File cache hit percentage for scan operators.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_hit_pct: Option<f64>,

    /// Rows filtered by bloom filter / zonemap / runtime filter at scan.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows_filtered: Option<f64>,

    /// Join type: shuffle, broadcast, colocated, bucket_shuffle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub join_type: Option<String>,
}
