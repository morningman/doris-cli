# doriscli end-to-end test harness

Black-box tests that drive the **built `doriscli` binary** against a **real,
already-deployed Doris cluster** and report which commands pass, fail, or are
skipped. Everything goes through `doriscli` itself — there is no separate MySQL
client and no mocking — so a green run means the CLI genuinely works end to end
against that cluster.

## TL;DR

```bash
# from the repo root (doris-cli/)
cp tests/e2e/cluster.env.example tests/e2e/cluster.env
$EDITOR tests/e2e/cluster.env          # fill in host/port/user/password
./start-testing.sh
```

or pass the connection on the command line:

```bash
./start-testing.sh --host fe.example.com --port 9030 --http-port 8030 \
                   --user root --password 'secret'
```

The runner builds `doriscli`, probes connectivity, creates a throwaway
`doriscli_selftest` database, runs every suite, **drops the database**, and
prints a summary. Exit code is `0` only if nothing **failed** (skips don't fail
the run). Full per-command output for the run is saved under
`tests/e2e/results/run-<timestamp>.log`.

Requirements: `bash`, `jq`, and either a Rust toolchain (to build) or a prebuilt
binary via `--bin`.

## Result semantics

| Status | Meaning |
|---|---|
| **PASS** | The command behaved exactly as the contract requires. |
| **FAIL** | The command misbehaved: wrong exit code, malformed JSON, or a missing/!= expected field. **These are the ones to look at.** |
| **SKIP** | A *precondition of the cluster* (not a bug in the CLI) was absent, so the test couldn't run — e.g. the FE HTTP API isn't reachable, or `audit_log` isn't enabled. Reported separately; never fails the run. |

The SKIP state exists because several `doriscli` features depend on optional
cluster capabilities. Treating "audit_log disabled" as a CLI failure would cry
wolf; treating it as a silent pass would hide untested surface. So it's its own
bucket, and the summary lists exactly what was skipped and why.

## What each suite covers

Run a subset with `--only "<suites>"`; list them with `--list`.

### `cargo test` (offline, unless `--no-unit`)
The crate's in-tree unit tests — primarily the profile-text parsers
(`section_parser`, `fragment_parser`, `operator_parser`, `value_parser`). No
cluster needed. Includes `parses_a_real_captured_profile`, an offline regression
that parses a **real** profile captured by the e2e run (see below) and asserts
the load-bearing invariants. It's a visible no-op until you commit the fixture
`tests/e2e/fixtures/sample_profile.txt` that a cluster run generates.

### `cli` — argument contract (offline)
`--version` / `-V`, `--help` (usage + subcommand listing), and the error paths:
`sql` with no query, unknown subcommand, `tablet` with no table, `profile` with
no action, and an unknown flag. Verifies non-zero exit on misuse.

### `auth` — connection management + stateless mode (needs cluster)
Uses an **isolated `$HOME`**, so your real `~/.doris` is never touched.
- `auth list` on an empty config → empty-list shape.
- `auth add` → saves an env (first one becomes default); `auth list` reflects it.
- `auth status` → **connects over MySQL**; asserts `.mysql_status == "connected"`
  (the command always exits 0, so the field is the real connectivity check).
- `use` / `use <name>` → show and switch the default env.
- `auth add --mysql mysql://…` → URI parsing (skipped if the password isn't
  URI-safe).
- `auth remove` → deletes an env.
- **Stateless mode** (`DORIS_HOST`+`DORIS_USER`): `auth add` is *refused*,
  `auth status` still connects from env vars, and **no files are written to
  disk** (verified against a pristine HOME).

### `sql` — execution (needs cluster)
The JSON envelope (`query_id`, `exec_time_ms`, `rows_returned`, `columns[]`,
`rows[]`), type mapping (string vs number), `-f <file>`, `--set` (single and
repeated), `--no-cache`, `--profile` (yields a `query_id`), `--format
table`/`csv`, empty result sets, a `COUNT(*)` over the loaded data, and the
error path (`SQL error:` on a bad reference, non-zero exit).

### `tablet` — bucket / tablet analysis (needs cluster + seeded data)
Overview: `model`, `bucket_type`, `bucket_key`, `bucket_count`, `sort_key`,
`total_rows`; the `health.tablet_skew` summary; `columns[].ndv` (SKIP if column
stats weren't collected). `UNIQUE` model detection on a second table.
`--detail` (per-tablet + per-backend) and `--detail --partition` (narrowed to
one partition). Negative: a missing table exits non-zero.

### `profile` — query profiles (needs cluster **+ the FE HTTP profile API**)
This suite is the point of the harness: it sends real SQL with `--profile`,
fetches the **real** profile, and asserts on the **parsed values**, not just the
JSON shape. It runs three profiled queries — a group-by over `events` (twice) and
a hash join `events⨝dim_users` — then checks, against the live profile:

- `profile list` / `list --active` → arrays carrying the real `SHOW QUERY
  PROFILE` fields (MySQL only, always testable).
- `profile get <id>` → the parsed Profile ID equals the id we sent, the parsed
  SQL is our query text, `total_time_ms` is a positive parsed number, the
  operator tree includes a `SCAN` and an `AGG`, `query_stats.total_scan_rows`
  is ≈ the rows we loaded, and the fragment breakdown is well-formed.
- `get --full` → the full `fragments→pipelines→operators` tree with populated
  `all_counters`; `get --raw` → the raw text round-trips (and is **captured as
  the offline regression fixture**, see below).
- `profile diff` → two real runs of the same query: parsed totals on both sides
  and a numeric `time_ratio`.
- the join query → a `JOIN` operator and ≥2 `SCAN` operators are parsed.

The full profile **text comes only over HTTP** (REST v2 / legacy; the SQL path
yields summary metadata with no operators). So the FE HTTP profile API is a hard
requirement: **if `http_status != connected`, these parse tests FAIL, not SKIP**
— a silent skip behind a green run would hide the entire parser. Two narrower
conditions still SKIP, because they're cluster preconditions and not parser bugs:
a profile **evicted** before we could fetch it (raise `max_query_profile_num`),
and `profile history`, which needs `__internal_schema.audit_log`. Operator→table
attribution auto-SKIPs on pre-4.0 Doris (its operator headers omit `table_name`).
A negative case (unknown query id exits non-zero) rounds it out.

## The seed data

`setup_data` creates two tables in `doriscli_selftest` (DDL is mode-agnostic;
`replication_num=1` works on self-hosted and is required in cloud mode):

- `events` — `DUPLICATE` model, range-partitioned by `event_date` into two
  partitions (Jan/Feb 2024), `DISTRIBUTED BY HASH(user_id) BUCKETS 4` → 8
  tablets. Loaded with `--rows` (default 2000) rows via a generated `INSERT`
  fed through `sql -f` (dates computed server-side by `DATE_ADD`, so no
  dependency on any particular Doris version). `ANALYZE TABLE … WITH SYNC` runs
  best-effort so `tablet`'s `columns[].ndv` is populated.
- `dim_users` — `UNIQUE` model, 2 buckets, a handful of rows.

The database is dropped on exit (even on Ctrl-C) unless you pass `--keep`.

## Options

See `./start-testing.sh --help`. Highlights: `--only`, `--no-unit`, `--keep`,
`--release`, `--bin <path>`, `--no-build`, `--rows <n>`, `--config <file>`.

## Notes on cluster prerequisites

- **MySQL/query port** (default 9030) is required for everything.
- **FE HTTP port** (default 8030; cloud often **8080**) enables `auth status`'s
  HTTP probe and is **required by the whole `profile` suite** — it's the only
  source of profile text, so without it the profile parse tests **FAIL** (they
  used to SKIP). Point `--http-port` at the right port before running `profile`.
- **Cloud / storage-compute**: set `DORIS_TEST_INIT_SQL='USE @<compute_group>'`
  so queries run against a live compute group; otherwise setup fails early with
  a hint.
- The test user needs `CREATE`/`DROP`/`INSERT` on the self-test database (and
  `SELECT` on `information_schema`). For full `profile` coverage the cluster
  should retain profiles (`enable_profile` is set per-query by `--profile`) and,
  for `profile history`, have the audit log enabled.
