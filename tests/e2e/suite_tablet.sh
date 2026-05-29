# shellcheck shell=bash
# suite_tablet — bucket/tablet analysis against the seeded tables.
# Overview keys verified against src/commands/tablet/overview.rs;
# --detail keys against src/commands/tablet/detail.rs (note: without --partition,
# detail attributes every tablet to the first partition name — asserted as such).

suite_tablet() {
  suite_banner "tablet  (model / bucket / health / detail)"
  DCLI_STATELESS=1
  local events="$CFG_DB.events"
  local dim="$CFG_DB.dim_users"

  # Overview: model, distribution, sort key, row count.
  expect_json "tablet: events overview (model/bucket/sortkey/rows)" \
    '.model=="DUPLICATE" and .bucket_type=="HASH" and (.bucket_key|index("user_id")) and (.bucket_count==4) and (.partitions>=2) and (.total_rows>0) and (.sort_key|index("event_date"))' \
    tablet "$events"

  # Health summary is computed from SHOW DATA SKEW (object with a numeric skew).
  expect_json "tablet: events health has a numeric tablet_skew" \
    '(.health|type=="object") and (.health.tablet_skew|type=="number")' \
    tablet "$events"

  # Column stats (ndv) require ANALYZE to have populated them — SKIP if absent.
  DCLI_STATELESS=1 _run_dcli --format json tablet "$events"
  if [ "$RC" -ne 0 ]; then
    record_fail "tablet: events columns[].ndv populated" "exit=$RC; $ERR"
  elif [ "$(jget '.columns|length')" = "0" ] || [ -z "$(jget '.columns|length')" ]; then
    record_skip "tablet: events columns[].ndv populated" "no column stats collected (ANALYZE async/unsupported)"
  elif printf '%s' "$OUT" | jq -e '.columns[0]|has("ndv")' >/dev/null 2>&1; then
    record_pass "tablet: events columns[].ndv populated"
  else
    record_fail "tablet: events columns[].ndv populated" "columns present but missing ndv: $(jget '.columns[0]')"
  fi

  # Model detection on the UNIQUE table.
  expect_json "tablet: dim_users is detected as UNIQUE" \
    '.model=="UNIQUE"' \
    tablet "$dim"

  # --detail: per-tablet + per-backend distribution. 2 partitions x 4 buckets = 8 tablets.
  expect_json "tablet: --detail lists tablets and backends" \
    '(.partitions|type=="array") and (.tablets|type=="array") and ((.tablets|length)>=8) and (.backends|type=="array")' \
    tablet "$events" --detail

  # --partition narrows to one partition's 4 tablets.
  expect_json "tablet: --detail --partition filters to one partition" \
    '((.partitions|length)==1) and (.partitions[0].name=="p20240101") and ((.tablets|length)==4)' \
    tablet "$events" --detail --partition p20240101

  # Negative: a missing table errors (SHOW CREATE TABLE fails).
  expect_err "tablet: missing table exits non-zero" \
    tablet "$CFG_DB.no_such_table_xyz"
}
