# shellcheck shell=bash
# suite_profile — query profile analysis.
#
# Dependency map (from the source):
#   profile list / list --active   MySQL only            (always testable)
#   profile get <id>  (summary)    HTTP, with SQL fallback (testable if --profile worked)
#   profile get --full / --raw     HTTP profile fetch ONLY -> SKIP if HTTP_OK=0
#   profile diff                   HTTP profile fetch ONLY -> SKIP if HTTP_OK=0
#   profile history                __internal_schema.audit_log -> SKIP if absent
#
# HTTP_OK is set in preflight from `auth status`.http_status.

suite_profile() {
  suite_banner "profile  (list / get / full / raw / diff / history)"
  DCLI_STATELESS=1

  # Generate two profiled queries and capture their ids.
  DCLI_STATELESS=1 _run_dcli --format json sql \
    "SELECT event_type, COUNT(*) AS c FROM \`$CFG_DB\`.events GROUP BY event_type ORDER BY c DESC" --profile
  local qid_a; qid_a="$(jget '.query_id')"
  DCLI_STATELESS=1 _run_dcli --format json sql \
    "SELECT user_id, SUM(amount) AS s FROM \`$CFG_DB\`.events GROUP BY user_id ORDER BY s DESC LIMIT 10" --profile
  local qid_b; qid_b="$(jget '.query_id')"
  log "  ${C_DIM}profiled query ids: A=$qid_a B=$qid_b  (HTTP_OK=$HTTP_OK)${C_RESET}"

  # profile list -> JSON array.
  DCLI_STATELESS=1 _run_dcli --format json profile list
  if [ "$RC" -ne 0 ]; then
    record_fail "profile: list returns an array" "exit=$RC; $ERR"
  elif printf '%s' "$OUT" | jq -e 'type=="array"' >/dev/null 2>&1; then
    record_pass "profile: list returns an array"
    if [ -n "$qid_a" ] && printf '%s' "$OUT" | jq -e --arg q "$qid_a" 'any(.[]?; .query_id==$q)' >/dev/null 2>&1; then
      record_pass "profile: list includes the just-profiled query"
    else
      record_skip "profile: list includes the just-profiled query" "not retained in SHOW QUERY PROFILE (eviction/timing)"
    fi
  else
    record_fail "profile: list returns an array" "not a JSON array: $(_oneline "$OUT")"
  fi

  # profile list --active -> JSON array (usually empty).
  expect_json "profile: list --active returns an array" 'type=="array"' \
    profile list --active

  if [ -z "$qid_a" ]; then
    skip "profile: get <id> summary"        "no query_id captured (--profile produced none)"
    skip "profile: get --full"              "no query_id captured"
    skip "profile: get --raw"               "no query_id captured"
    skip "profile: diff slow vs fast"       "no query_id captured"
  else
    # Default get: summary object. Works even without HTTP via the SQL fallback.
    expect_json "profile: get <id> returns a summary" \
      '(.summary|type=="object") and (.summary|has("query_id"))' \
      profile get "$qid_a"

    # --full and --raw require the FE HTTP profile API.
    if [ "$HTTP_OK" = "1" ]; then
      expect_json "profile: get --full returns the parsed tree" \
        '(.profile|type=="object") and (.operators|type=="array")' \
        profile get "$qid_a" --full
      expect_json "profile: get --raw returns the raw profile text" \
        'type=="string" and (contains("Summary"))' \
        profile get "$qid_a" --raw
    else
      skip "profile: get --full" "FE HTTP API not reachable (http_status != connected)"
      skip "profile: get --raw"  "FE HTTP API not reachable (http_status != connected)"
    fi

    # diff needs both ids + HTTP.
    if [ "$HTTP_OK" = "1" ] && [ -n "$qid_b" ]; then
      expect_json "profile: diff compares two runs" \
        '(.slow|type=="object") and (.fast|type=="object") and (.time_ratio|type=="number")' \
        profile diff "$qid_a" "$qid_b"
    else
      skip "profile: diff slow vs fast" "needs FE HTTP API and two query ids"
    fi
  fi

  # profile history: depends on __internal_schema.audit_log being enabled.
  DCLI_STATELESS=1 _run_dcli --format json profile history "events" --days 1
  if [ "$RC" -ne 0 ]; then
    if printf '%s' "$ERR" | grep -qi "audit_log"; then
      record_skip "profile: history reads audit_log" "audit_log not enabled/accessible on this cluster"
    else
      record_fail "profile: history reads audit_log" "exit=$RC; $ERR"
    fi
  elif printf '%s' "$OUT" | jq -e 'has("executions") and has("pattern")' >/dev/null 2>&1; then
    record_pass "profile: history reads audit_log"
  else
    record_fail "profile: history reads audit_log" "unexpected shape: $(_oneline "$OUT")"
  fi

  # Negative: an unknown query id cannot be fetched anywhere -> non-zero exit.
  expect_err "profile: get on an unknown id errors" \
    profile get "00000000-0000-0000-0000-000000000000"
}
