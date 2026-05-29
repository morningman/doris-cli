# shellcheck shell=bash
# tests/e2e/lib.sh — shared helpers for the doriscli end-to-end test harness.
#
# Sourced by start-testing.sh (which runs under bash). Provides:
#   - result tracking (PASS / FAIL / SKIP) and the final summary
#   - _run_dcli: the single place that invokes the doriscli binary, in either
#       * stateless mode  (DORIS_HOST/USER/... exported — no config files touched), or
#       * file mode       (DORIS_* unset, isolated $HOME — exercises ~/.doris config)
#   - assertion helpers built on jq: expect_json / expect_ok / expect_err / skip
#
# Design notes that the assertions depend on (verified against doriscli source):
#   - Every command prints a bare JSON value to stdout and exits 0 on success,
#     non-zero on error (error text on stderr).
#   - `auth status` ALWAYS exits 0; real connectivity is in the .mysql_status field.
#   - profile get --full/--raw and profile diff REQUIRE the FE HTTP API; the default
#     `profile get` summary has a SQL fallback; `profile history` needs audit_log.
#     Those are gated on HTTP_OK / detected at runtime and recorded as SKIP, not FAIL.

# ---- result state --------------------------------------------------------
N_PASS=0
N_FAIL=0
N_SKIP=0
FAILED_NAMES=()
SKIPPED_NAMES=()

# Populated by _run_dcli on every call:
OUT=""   # captured stdout
ERR=""   # captured stderr
RC=0     # exit code

# ---- colors (disabled when not a tty or NO_COLOR set) --------------------
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  C_GREEN=$'\033[32m'; C_RED=$'\033[31m'; C_YELLOW=$'\033[33m'
  C_BOLD=$'\033[1m'; C_DIM=$'\033[2m'; C_RESET=$'\033[0m'
else
  C_GREEN=""; C_RED=""; C_YELLOW=""; C_BOLD=""; C_DIM=""; C_RESET=""
fi

# ---- logging -------------------------------------------------------------
# log: to console AND the run log file. logfile_only: just the file.
log()          { printf '%s\n' "$*"; printf '%s\n' "$*" >>"$LOG_FILE" 2>/dev/null || true; }
logfile_only() { printf '%s\n' "$*" >>"$LOG_FILE" 2>/dev/null || true; }

# Collapse a (possibly multi-line) string to a single, truncated line for the
# console summary. Full untruncated output always lives in $LOG_FILE.
_oneline() {
  printf '%s' "$1" | tr '\n\t' '  ' | cut -c1-240
}

# ---- result recorders ----------------------------------------------------
record_pass() {
  N_PASS=$((N_PASS + 1))
  log "  ${C_GREEN}PASS${C_RESET} $1"
}
record_fail() {
  N_FAIL=$((N_FAIL + 1))
  FAILED_NAMES+=("$1")
  log "  ${C_RED}FAIL${C_RESET} $1"
  [ -n "$2" ] && log "       ${C_DIM}↳ $(_oneline "$2")${C_RESET}"
}
record_skip() {
  N_SKIP=$((N_SKIP + 1))
  SKIPPED_NAMES+=("$1")
  log "  ${C_YELLOW}SKIP${C_RESET} $1"
  [ -n "$2" ] && log "       ${C_DIM}↳ $(_oneline "$2")${C_RESET}"
}

# Print a section banner for a suite.
suite_banner() {
  log ""
  log "${C_BOLD}━━ $1 ━━${C_RESET}"
}

# ---- the doriscli runner -------------------------------------------------
# _run_dcli <args...>
#   Runs $BIN with the configured global flags (--init-sql / --socks5) prepended,
#   in stateless mode by default. Set DCLI_STATELESS=0 before calling to exercise
#   the file-based config path (DORIS_* unset, $HOME isolated to $ISOLATED_HOME).
#   Captures stdout->OUT, stderr->ERR, exit code->RC. Logs the invocation.
_run_dcli() {
  local globals=()
  [ -n "${CFG_INIT_SQL:-}" ] && globals+=(--init-sql "$CFG_INIT_SQL")
  [ -n "${CFG_SOCKS5:-}" ]   && globals+=(--socks5 "$CFG_SOCKS5")

  local errf; errf="$(mktemp "${TMPDIR:-/tmp}/dcli-err.XXXXXX")"

  if [ "${DCLI_STATELESS:-1}" = "1" ]; then
    OUT=$(HOME="$ISOLATED_HOME" \
          DORIS_HOST="$CFG_HOST" \
          DORIS_PORT="$CFG_PORT" \
          DORIS_HTTP_PORT="$CFG_HTTP_PORT" \
          DORIS_USER="$CFG_USER" \
          DORIS_PASSWORD="$CFG_PASSWORD" \
          "$BIN" "${globals[@]}" "$@" 2>"$errf")
    RC=$?
  else
    # File mode: scrub any DORIS_* the caller's shell may carry, so stateless
    # mode is NOT triggered, and point HOME at the isolated config dir.
    OUT=$(HOME="$ISOLATED_HOME" \
          env -u DORIS_HOST -u DORIS_USER -u DORIS_PASSWORD \
              -u DORIS_PORT -u DORIS_HTTP_PORT -u DORIS_ENV -u DORIS_INIT_SQL \
          "$BIN" "${globals[@]}" "$@" 2>"$errf")
    RC=$?
  fi

  ERR=$(cat "$errf" 2>/dev/null); rm -f "$errf"

  {
    printf '\n$ doriscli'
    printf ' %q' "${globals[@]}" "$@"
    printf '   [stateless=%s]\n' "${DCLI_STATELESS:-1}"
    printf 'rc=%s\n--- stdout ---\n%s\n--- stderr ---\n%s\n' "$RC" "$OUT" "$ERR"
  } >>"$LOG_FILE" 2>/dev/null || true
}

# ---- assertion helpers ---------------------------------------------------
# All of these record exactly one PASS or FAIL (or SKIP) result.

# expect_json "<name>" '<jq filter>' <doriscli args...>
#   Appends --format json, expects exit 0 and a jq filter that evaluates truthy.
#   Pass '' as the filter to only require valid JSON + exit 0.
expect_json() {
  local name="$1" filter="$2"; shift 2
  _run_dcli --format json "$@"
  if [ "$RC" -ne 0 ]; then
    record_fail "$name" "exit=$RC; stderr: $ERR"
    return
  fi
  if ! printf '%s' "$OUT" | jq -e . >/dev/null 2>&1; then
    record_fail "$name" "stdout is not valid JSON: $OUT"
    return
  fi
  if [ -n "$filter" ] && ! printf '%s' "$OUT" | jq -e "$filter" >/dev/null 2>&1; then
    record_fail "$name" "assertion failed [ $filter ]; got: $OUT"
    return
  fi
  record_pass "$name"
}

# expect_ok "<name>" <doriscli args...>  — just requires exit 0 (raw, no --format).
expect_ok() {
  local name="$1"; shift
  _run_dcli "$@"
  if [ "$RC" -eq 0 ]; then record_pass "$name"
  else record_fail "$name" "exit=$RC; stderr: $ERR"; fi
}

# expect_err "<name>" <doriscli args...>  — requires NON-zero exit.
#   Optional: set EXPECT_ERR_MATCH to a substring required in stderr.
expect_err() {
  local name="$1"; shift
  _run_dcli "$@"
  if [ "$RC" -eq 0 ]; then
    record_fail "$name" "expected non-zero exit, got 0; stdout: $OUT"
    return
  fi
  if [ -n "${EXPECT_ERR_MATCH:-}" ] && ! printf '%s' "$ERR" | grep -qiF "$EXPECT_ERR_MATCH"; then
    record_fail "$name" "exit ok but stderr missing '$EXPECT_ERR_MATCH': $ERR"
    return
  fi
  record_pass "$name"
}

# expect_stdout_contains "<name>" "<substr>" <doriscli args...>
#   Raw run (no --format), exit 0, and stdout contains substr (for --help, table/csv).
expect_stdout_contains() {
  local name="$1" needle="$2"; shift 2
  _run_dcli "$@"
  if [ "$RC" -ne 0 ]; then
    record_fail "$name" "exit=$RC; stderr: $ERR"
    return
  fi
  if printf '%s' "$OUT" | grep -qF "$needle"; then record_pass "$name"
  else record_fail "$name" "stdout missing '$needle': $(_oneline "$OUT")"; fi
}

# skip "<name>" "<reason>" — record a SKIP (precondition absent, not a bug).
skip() { record_skip "$1" "$2"; }

# jget '<jq filter>' — echo a scalar pulled from the last OUT (raw -r). Empty on error.
jget() { printf '%s' "$OUT" | jq -r "$1" 2>/dev/null; }

# ---- final summary -------------------------------------------------------
print_summary() {
  local total=$((N_PASS + N_FAIL + N_SKIP))
  log ""
  log "${C_BOLD}══════════════════════ SUMMARY ══════════════════════${C_RESET}"
  log "  total: $total    ${C_GREEN}pass: $N_PASS${C_RESET}    ${C_RED}fail: $N_FAIL${C_RESET}    ${C_YELLOW}skip: $N_SKIP${C_RESET}"
  if [ "$N_SKIP" -gt 0 ]; then
    log ""
    log "${C_YELLOW}Skipped (precondition not met on this cluster — not a failure):${C_RESET}"
    local s; for s in "${SKIPPED_NAMES[@]}"; do log "  - $s"; done
  fi
  if [ "$N_FAIL" -gt 0 ]; then
    log ""
    log "${C_RED}${C_BOLD}Failed tests:${C_RESET}"
    local f; for f in "${FAILED_NAMES[@]}"; do log "  ${C_RED}✗${C_RESET} $f"; done
    log ""
    log "Full command output for every test is in:"
    log "  $LOG_FILE"
    log "${C_RED}${C_BOLD}RESULT: FAIL ($N_FAIL failing)${C_RESET}"
  else
    log ""
    log "Full run log: $LOG_FILE"
    log "${C_GREEN}${C_BOLD}RESULT: PASS${C_RESET}"
  fi
}
