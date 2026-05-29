# shellcheck shell=bash
# tests/e2e/data.sh — create / load / drop the self-test database.
#
# Everything runs THROUGH doriscli itself (no external mysql client), so loading
# data also exercises `sql` (incl. the -f file path) against the real cluster.
# All DDL is mode-agnostic (replication_num=1 works on self-hosted single/multi-BE
# and is required in cloud / storage-compute mode).

SETUP_DB_CREATED=0

# Abort the whole run with an actionable hint. Teardown still fires via the trap.
die_setup() {
  log ""
  log "${C_RED}${C_BOLD}SETUP FAILED${C_RESET} — ${1}"
  log "  ${C_DIM}stderr: $(_oneline "$ERR")${C_RESET}"
  log ""
  log "Likely causes:"
  log "  • Wrong host/port/user/password — check your cluster.env / flags."
  log "  • Cloud (storage-compute) cluster needs a compute group selected first:"
  log "      set  DORIS_TEST_INIT_SQL='USE @<your_compute_group>'  in cluster.env"
  log "  • The user lacks CREATE / DROP / LOAD / INSERT privileges."
  log "  • FE query port unreachable from this host."
  exit 1
}

# Emit a single multi-row INSERT for the events table (no `numbers` TVF dependency;
# dates are computed server-side by DATE_ADD so it works on any Doris version).
_gen_events_sql() {
  local db="$1" n="${2:-2000}"
  awk -v db="$db" -v n="$n" 'BEGIN {
    split("click,view,purchase,signup", t, ",");
    printf "INSERT INTO `%s`.`events` (event_date,user_id,event_type,amount,detail) VALUES\n", db;
    for (i = 0; i < n; i++) {
      day = i % 59;            # 0..58 days -> spans Jan + Feb 2024 (both partitions)
      uid = i % 1000;          # cardinality ~1000 for user_id
      typ = t[(i % 4) + 1];
      amt = (i % 100) + 0.5;
      sep = (i == n - 1) ? ";" : ",";
      printf "(DATE_ADD(\x272024-01-01\x27, INTERVAL %d DAY),%d,\x27%s\x27,%.2f,\x27detail_%d\x27)%s\n",
             day, uid, typ, amt, i, sep;
    }
  }'
}

setup_data() {
  suite_banner "SETUP  (database '$CFG_DB' on the target cluster)"
  local db="$CFG_DB"

  # Fresh start: drop a leftover db from a previous interrupted run.
  _run_dcli --format json sql "DROP DATABASE IF EXISTS \`$db\`"
  [ "$RC" -eq 0 ] || die_setup "could not DROP a pre-existing '$db' (connectivity / privileges?)"

  _run_dcli --format json sql "CREATE DATABASE \`$db\`"
  [ "$RC" -eq 0 ] || die_setup "CREATE DATABASE \`$db\` failed"
  SETUP_DB_CREATED=1
  log "  created database $db"

  # --- events: DUPLICATE, range-partitioned, HASH bucketed (drives `tablet`) ---
  _run_dcli --format json sql "CREATE TABLE \`$db\`.\`events\` (
      event_date DATE NOT NULL,
      user_id    BIGINT NOT NULL,
      event_type VARCHAR(32) NOT NULL,
      amount     DECIMAL(10,2),
      detail     STRING
    )
    DUPLICATE KEY(event_date, user_id)
    PARTITION BY RANGE(event_date) (
      PARTITION p20240101 VALUES [('2024-01-01'), ('2024-02-01')),
      PARTITION p20240201 VALUES [('2024-02-01'), ('2024-03-01'))
    )
    DISTRIBUTED BY HASH(user_id) BUCKETS 4
    PROPERTIES ('replication_num' = '1')"
  [ "$RC" -eq 0 ] || die_setup "CREATE TABLE events failed"
  log "  created table events (DUPLICATE, 2 partitions x 4 buckets)"

  # --- dim_users: UNIQUE model (so `tablet` model detection is exercised twice) ---
  _run_dcli --format json sql "CREATE TABLE \`$db\`.\`dim_users\` (
      user_id BIGINT NOT NULL,
      name    VARCHAR(64),
      level   INT
    )
    UNIQUE KEY(user_id)
    DISTRIBUTED BY HASH(user_id) BUCKETS 2
    PROPERTIES ('replication_num' = '1')"
  [ "$RC" -eq 0 ] || die_setup "CREATE TABLE dim_users failed"
  log "  created table dim_users (UNIQUE, 2 buckets)"

  # --- load events via a generated .sql file (exercises `sql -f`) ---
  local events_sql="$WORKDIR/events_insert.sql"
  _gen_events_sql "$db" "${ROWS:-2000}" >"$events_sql"
  _run_dcli --format json sql -f "$events_sql"
  [ "$RC" -eq 0 ] || die_setup "loading events via 'sql -f' failed"
  log "  loaded ${ROWS:-2000} rows into events (via sql -f)"

  # --- load dim_users (small inline INSERT) ---
  _run_dcli --format json sql "INSERT INTO \`$db\`.\`dim_users\` (user_id,name,level) VALUES
      (1,'alice',3),(2,'bob',1),(3,'carol',2),(4,'dave',5),(5,'erin',4),
      (6,'frank',2),(7,'grace',1),(8,'heidi',3),(9,'ivan',2),(10,'judy',4)"
  [ "$RC" -eq 0 ] || die_setup "loading dim_users failed"
  log "  loaded 10 rows into dim_users"

  # --- column stats so tablet's columns[].ndv is populated (best effort) ---
  # ANALYZE may be async/unavailable on some versions; failure is not fatal —
  # the tablet suite degrades the column-stats check to SKIP if stats are absent.
  _run_dcli --format json sql "ANALYZE TABLE \`$db\`.\`events\` WITH SYNC"
  if [ "$RC" -eq 0 ]; then
    log "  ran ANALYZE TABLE events WITH SYNC (column stats collected)"
  else
    log "  ${C_YELLOW}note${C_RESET}: ANALYZE TABLE events did not succeed; columns[].ndv may be empty"
  fi

  # --- wait for partition RowCount to be reported (drives tablet total_rows) ---
  # Doris updates SHOW PARTITIONS' RowCount asynchronously (BE->FE tablet report),
  # so right after a load it can still read 0 while the rows are already queryable.
  # The tablet suite asserts total_rows>0 (summed from SHOW PARTITIONS), so poll the
  # exact same value until it materializes. Bounded by ROWCOUNT_TIMEOUT (default
  # 120s); on timeout we proceed and let the assertion surface the lag, never hang.
  local rc_timeout="${ROWCOUNT_TIMEOUT:-120}" rc_waited=0 rc_rows="" rc_ok=0
  while :; do
    _run_dcli --format json tablet "$db.events"
    rc_rows="$(jget '.total_rows')"
    case "$rc_rows" in [1-9]*) rc_ok=1; break;; esac
    [ "$rc_waited" -ge "$rc_timeout" ] && break
    sleep 3; rc_waited=$((rc_waited + 3))
  done
  if [ "$rc_ok" = 1 ]; then
    log "  events RowCount reported (total_rows=$rc_rows after ${rc_waited}s)"
  else
    log "  ${C_YELLOW}note${C_RESET}: events total_rows still 0 after ${rc_waited}s — tablet 'total_rows>0' may FAIL (SHOW PARTITIONS RowCount report lag)"
  fi

  log "${C_GREEN}  setup complete${C_RESET}"
}

teardown_data() {
  # Called from the EXIT trap, so guard everything and never abort.
  [ "${SETUP_DB_CREATED:-0}" = "1" ] || return 0
  if [ "${KEEP_DB:-0}" = "1" ]; then
    log ""
    log "${C_YELLOW}--keep set: leaving database '$CFG_DB' in place.${C_RESET}"
    return 0
  fi
  log ""
  log "Teardown: dropping database '$CFG_DB' ..."
  _run_dcli --format json sql "DROP DATABASE IF EXISTS \`$CFG_DB\`"
  if [ "$RC" -eq 0 ]; then log "  dropped $CFG_DB"
  else log "  ${C_YELLOW}warning${C_RESET}: could not drop $CFG_DB — drop it manually. ($(_oneline "$ERR"))"; fi
}
