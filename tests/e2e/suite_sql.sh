# shellcheck shell=bash
# suite_sql — SQL execution surface: the JSON envelope, -f files, --set session
# vars, --no-cache, --profile, the three output formats, and the error path.
# Runs in stateless mode (env-var connection; no config files).

suite_sql() {
  suite_banner "sql  (execution, session vars, formats, errors)"
  DCLI_STATELESS=1

  # Envelope shape: query_id, exec_time_ms, rows_returned, columns[], rows[].
  expect_json "sql: simple select returns the full JSON envelope" \
    '.rows[0].one==1 and (.columns|index("one")) and (.query_id|type=="string") and (.rows_returned==1) and (.exec_time_ms>=0)' \
    sql "SELECT 1 AS one"

  # Mixed types: string stays a string, integer becomes a JSON number.
  expect_json "sql: string and integer columns map correctly" \
    '.rows[0].g=="hello" and .rows[0].n==42' \
    sql "SELECT 'hello' AS g, 42 AS n"

  # -f <file>: read the query from a file.
  printf 'SELECT 7 AS seven' > "$WORKDIR/seven.sql"
  expect_json "sql: -f reads the query from a file" \
    '.rows[0].seven==7' \
    sql -f "$WORKDIR/seven.sql"

  # --set applies a session var before the query (integer var avoids normalization).
  expect_json "sql: --set applies a session variable" \
    '.rows[0].p==5' \
    sql "SELECT @@parallel_pipeline_task_num AS p" --set "parallel_pipeline_task_num=5"

  # --set is repeatable.
  expect_json "sql: multiple --set flags all apply" \
    '.rows[0].p==6 and .rows[0].w==2000' \
    sql "SELECT @@parallel_pipeline_task_num AS p, @@runtime_filter_wait_time_ms AS w" \
      --set "parallel_pipeline_task_num=6" --set "runtime_filter_wait_time_ms=2000"

  # --no-cache: just confirm it executes cleanly.
  expect_json "sql: --no-cache executes" \
    '.rows_returned==1' \
    sql "SELECT 1 AS one" --no-cache

  # --profile: produces a real Doris query id (TUniqueId rendered as hex-hi-hex-lo,
  # e.g. "a1b2c3d4...-e5f6..."), reused by the profile suite to fetch the profile.
  expect_json "sql: --profile yields a real Doris query_id" \
    '(.query_id|type=="string") and (.query_id|test("^[0-9a-fA-F]+-[0-9a-fA-F]+$"))' \
    sql "SELECT 2 AS two" --profile

  # Output formats: a TTY-style table and CSV both carry the column header.
  expect_stdout_contains "sql: --format table renders the column" "one" \
    sql "SELECT 1 AS one" --format table
  expect_stdout_contains "sql: --format csv renders the header" "one" \
    sql "SELECT 1 AS one" --format csv

  # Empty result set: zero rows, empty rows array, still exit 0.
  expect_json "sql: empty result set is handled" \
    '.rows_returned==0 and (.rows==[])' \
    sql "SELECT table_name FROM information_schema.tables WHERE 1=0"

  # End-to-end against loaded data: row count matches what setup loaded, and the
  # envelope reports a real (non-negative) execution time for a query that ran.
  expect_json "sql: count over loaded table matches the load" \
    '.rows[0].c=='"${ROWS:-2000}"' and (.exec_time_ms|type=="number") and (.exec_time_ms>=0)' \
    sql "SELECT COUNT(*) AS c FROM \`$CFG_DB\`.events"

  # Error path: a bad reference exits non-zero with an "SQL error:" message.
  EXPECT_ERR_MATCH="SQL error"
  expect_err "sql: invalid query exits non-zero" \
    sql "SELECT * FROM ${CFG_DB}.no_such_table_xyz"
  EXPECT_ERR_MATCH=""
}
