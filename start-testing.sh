#!/usr/bin/env bash
# start-testing.sh — end-to-end test runner for the doriscli binary.
#
# Point it at a deployed Doris cluster and it builds doriscli, exercises every
# command against the cluster, and prints which tests passed / failed / skipped.
#
#   ./start-testing.sh --host fe.example.com --port 9030 --http-port 8030 \
#                      --user root --password 'secret'
#
# or put the connection in tests/e2e/cluster.env (see cluster.env.example) and run:
#
#   ./start-testing.sh
#
# Exit code is 0 only when nothing FAILED (skips do not fail the run).
# Run `./start-testing.sh --help` for all options.

# Re-exec under bash if invoked as `sh start-testing.sh` on a system where /bin/sh
# is not bash (the harness uses bash arrays/locals).
if [ -z "${BASH_VERSION:-}" ]; then exec bash "$0" "$@"; fi

set -o pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$SCRIPT_DIR"
E2E_DIR="$SCRIPT_DIR/tests/e2e"

# ---- defaults ------------------------------------------------------------
RUN_UNIT=1
KEEP_DB=0
NO_BUILD=0
BUILD_PROFILE="debug"
LIST_ONLY=0
ALL_SUITES="cli auth sql tablet profile"
ONLY_SUITES=""
CONFIG_FILE="$E2E_DIR/cluster.env"
CONFIG_FILE_EXPLICIT=0

usage() {
  cat <<'EOF'
Usage: ./start-testing.sh [connection options] [runner options]

Connection (overrides tests/e2e/cluster.env; can also come from that file):
  --host <h>          FE host (required, unless set in cluster.env)
  --port <n>          MySQL/query port            (default 9030)
  --http-port <n>     FE HTTP port                (default 8030; cloud often 8080)
  --user <u>          MySQL user                  (default root)
  --password <p>      MySQL password              (default empty)
  --init-sql <sql>    Run after connect, e.g. 'USE @<compute_group>' for cloud
  --socks5 <u:p@h:p>  Route through a SOCKS5 proxy (BYOC)
  --db <name>         Self-test database name     (default doriscli_selftest)
  --rows <n>          Rows to load into events    (default 2000)

Runner:
  --config <file>     Connection file to source   (default tests/e2e/cluster.env)
  --bin <path>        Use a prebuilt doriscli instead of building
  --release           Build the release binary instead of debug
  --no-build          Do not build; use an existing target/ binary or --bin
  --no-unit           Skip the offline `cargo test` unit suite
  --only "<suites>"   Run only these suites (space/comma list): cli auth sql tablet profile
  --keep              Do not drop the self-test database afterwards
  --list              List the suites and exit
  -h, --help          Show this help

Examples:
  ./start-testing.sh --host 127.0.0.1 --user root --password ''
  ./start-testing.sh --only "cli sql" --no-unit
  ./start-testing.sh --host fe --http-port 8080 --init-sql 'USE @my_cg'   # cloud
EOF
}

# ---- parse args ----------------------------------------------------------
while [ $# -gt 0 ]; do
  case "$1" in
    --host)       FLAG_HOST="$2"; shift 2;;
    --port)       FLAG_PORT="$2"; shift 2;;
    --http-port)  FLAG_HTTP_PORT="$2"; shift 2;;
    --user)       FLAG_USER="$2"; shift 2;;
    --password)   FLAG_PASSWORD="$2"; shift 2;;
    --init-sql)   FLAG_INIT_SQL="$2"; shift 2;;
    --socks5)     FLAG_SOCKS5="$2"; shift 2;;
    --db)         FLAG_DB="$2"; shift 2;;
    --rows)       FLAG_ROWS="$2"; shift 2;;
    --config)     CONFIG_FILE="$2"; CONFIG_FILE_EXPLICIT=1; shift 2;;
    --bin)        BIN_OVERRIDE="$2"; shift 2;;
    --release)    BUILD_PROFILE="release"; shift;;
    --no-build)   NO_BUILD=1; shift;;
    --no-unit)    RUN_UNIT=0; shift;;
    --only)       ONLY_SUITES="$2"; shift 2;;
    --keep)       KEEP_DB=1; shift;;
    --list)       LIST_ONLY=1; shift;;
    -h|--help)    usage; exit 0;;
    *) echo "Unknown option: $1" >&2; echo; usage; exit 2;;
  esac
done

if [ "$LIST_ONLY" = 1 ]; then
  echo "Available suites: $ALL_SUITES"
  echo "  cli      offline CLI contract (version/help/argument errors)"
  echo "  auth     auth add/list/status/remove, use, stateless mode (needs cluster)"
  echo "  sql      query execution, -f, --set, --no-cache, --profile, formats, errors"
  echo "  tablet   model/bucket/health/detail (needs cluster + seeded data)"
  echo "  profile  list/get/full/raw/diff/history (needs cluster; some auto-SKIP)"
  echo "Plus an offline 'cargo test' unit suite unless --no-unit."
  exit 0
fi

# ---- resolve connection (CLI flag > cluster.env > default) ---------------
if [ -f "$CONFIG_FILE" ]; then
  # shellcheck disable=SC1090
  . "$CONFIG_FILE"
elif [ "$CONFIG_FILE_EXPLICIT" = 1 ]; then
  echo "Config file not found: $CONFIG_FILE" >&2; exit 2
fi

CFG_HOST="${FLAG_HOST:-${DORIS_TEST_HOST:-}}"
CFG_PORT="${FLAG_PORT:-${DORIS_TEST_PORT:-9030}}"
CFG_HTTP_PORT="${FLAG_HTTP_PORT:-${DORIS_TEST_HTTP_PORT:-8030}}"
CFG_USER="${FLAG_USER:-${DORIS_TEST_USER:-root}}"
CFG_PASSWORD="${FLAG_PASSWORD-${DORIS_TEST_PASSWORD-}}"
CFG_INIT_SQL="${FLAG_INIT_SQL-${DORIS_TEST_INIT_SQL-}}"
CFG_SOCKS5="${FLAG_SOCKS5-${DORIS_TEST_SOCKS5-}}"
CFG_DB="${FLAG_DB:-${DORIS_TEST_DB:-doriscli_selftest}}"
ROWS="${FLAG_ROWS:-${DORIS_TEST_ROWS:-2000}}"

# Which suites, and do any of them require the cluster?
SUITES="${ONLY_SUITES:-$ALL_SUITES}"
SUITES="$(printf '%s' "$SUITES" | tr ',' ' ')"
wants() { case " $SUITES " in *" $1 "*) return 0;; *) return 1;; esac; }

needs_cluster=0
for s in auth sql tablet profile; do wants "$s" && needs_cluster=1; done

if [ "$needs_cluster" = 1 ] && [ -z "$CFG_HOST" ]; then
  echo "No cluster connection provided." >&2
  echo "Pass --host ... (see --help) or create $E2E_DIR/cluster.env" >&2
  echo "from cluster.env.example. (Or run only the offline suite: --only cli)" >&2
  exit 2
fi

# ---- preflight: dependencies + binary ------------------------------------
command -v jq >/dev/null 2>&1 || { echo "Missing dependency: jq (install jq and retry)." >&2; exit 2; }

if [ -n "${BIN_OVERRIDE:-}" ]; then
  BIN="$BIN_OVERRIDE"
else
  REL="$REPO_ROOT/target/release/doriscli"
  DBG="$REPO_ROOT/target/debug/doriscli"
  if [ "$NO_BUILD" = 1 ]; then
    if   [ -x "$REL" ]; then BIN="$REL"
    elif [ -x "$DBG" ]; then BIN="$DBG"
    else echo "--no-build set but no binary in target/. Build it or pass --bin." >&2; exit 2; fi
  else
    command -v cargo >/dev/null 2>&1 || { echo "cargo not found; pass --bin <path> or install Rust." >&2; exit 2; }
    echo "Building doriscli ($BUILD_PROFILE) — first build may take a few minutes ..."
    if [ "$BUILD_PROFILE" = "release" ]; then
      cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml" || { echo "build failed" >&2; exit 2; }
      BIN="$REL"
    else
      cargo build --manifest-path "$REPO_ROOT/Cargo.toml" || { echo "build failed" >&2; exit 2; }
      BIN="$DBG"
    fi
  fi
fi
[ -x "$BIN" ] || { echo "doriscli binary not found/executable: $BIN" >&2; exit 2; }

# ---- working dirs + logging ----------------------------------------------
RESULTS_DIR="$E2E_DIR/results"
mkdir -p "$RESULTS_DIR"
TS="$(date +%Y%m%d-%H%M%S)"
LOG_FILE="$RESULTS_DIR/run-$TS.log"
WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/doriscli-e2e.XXXXXX")"
ISOLATED_HOME="$WORKDIR/home"
mkdir -p "$ISOLATED_HOME"

# Export for the helpers/suites.
export BIN CFG_HOST CFG_PORT CFG_HTTP_PORT CFG_USER CFG_PASSWORD CFG_INIT_SQL CFG_SOCKS5 CFG_DB ROWS
export LOG_FILE WORKDIR ISOLATED_HOME REPO_ROOT KEEP_DB HTTP_OK

# ---- source helpers + suites ---------------------------------------------
# shellcheck source=tests/e2e/lib.sh
. "$E2E_DIR/lib.sh"
# shellcheck source=tests/e2e/data.sh
. "$E2E_DIR/data.sh"
for f in cli auth sql tablet profile; do
  # shellcheck disable=SC1090
  . "$E2E_DIR/suite_$f.sh"
done

# Offline unit suite (defined here; needs cargo + source tree).
suite_unit() {
  suite_banner "cargo unit tests (offline)"
  if ! command -v cargo >/dev/null 2>&1; then
    record_skip "cargo test (unit)" "cargo not found"
    return
  fi
  log "  running 'cargo test' (output in the run log) ..."
  printf '\n===== cargo test =====\n' >>"$LOG_FILE"
  if cargo test --manifest-path "$REPO_ROOT/Cargo.toml" >>"$LOG_FILE" 2>&1; then
    record_pass "cargo test (unit)"
  else
    record_fail "cargo test (unit)" "see the 'cargo test' section in $LOG_FILE"
  fi
}

# ---- teardown trap (drops the self-test DB, cleans temp) -----------------
cleanup() {
  type teardown_data >/dev/null 2>&1 && teardown_data
  rm -rf "$WORKDIR" 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# ---- header --------------------------------------------------------------
HTTP_OK=0
log "${C_BOLD}doriscli end-to-end test run${C_RESET}  ($TS)"
log "  binary : $BIN"
log "  version: $("$BIN" --version 2>/dev/null)"
unit_note=""; [ "$RUN_UNIT" = 1 ] && unit_note="  (+cargo unit)"
log "  suites : $SUITES$unit_note"
log "  log    : $LOG_FILE"
if [ "$needs_cluster" = 1 ]; then
  log "  cluster: $CFG_USER@$CFG_HOST:$CFG_PORT (http :$CFG_HTTP_PORT)  db=$CFG_DB"
  [ -n "$CFG_INIT_SQL" ] && log "  init-sql: $CFG_INIT_SQL"
fi

# ---- offline suites first (fast feedback, no cluster) --------------------
[ "$RUN_UNIT" = 1 ] && suite_unit
wants cli && suite_cli

# ---- connectivity probe (gates the cluster suites; sets HTTP_OK) ---------
if [ "$needs_cluster" = 1 ]; then
  suite_banner "Connectivity probe"
  DCLI_STATELESS=1 _run_dcli --format json auth status
  MS="$(jget '.mysql_status')"
  if [ "$MS" != "connected" ]; then
    log "${C_RED}${C_BOLD}Cannot connect to the cluster — aborting cluster suites.${C_RESET}"
    log "  mysql_status: $MS"
    log "  stderr: $(_oneline "$ERR")"
    log "  Verify host/port/user/password. For a cloud/storage-compute cluster, set"
    log "  DORIS_TEST_INIT_SQL='USE @<compute_group>' and --http-port (often 8080)."
    print_summary
    exit 1
  fi
  [ "$(jget '.http_status')" = "connected" ] && HTTP_OK=1
  log "  ${C_GREEN}connected${C_RESET}  version=$(jget '.doris_version')  http_status=$(jget '.http_status')  backends=$(jget '.backends|length')  (HTTP_OK=$HTTP_OK)"
fi

# ---- cluster suites ------------------------------------------------------
wants auth && suite_auth

if wants sql || wants tablet || wants profile; then
  setup_data
fi
wants sql     && suite_sql
wants tablet  && suite_tablet
wants profile && suite_profile

# ---- summary + exit code -------------------------------------------------
print_summary
[ "$N_FAIL" -eq 0 ]
