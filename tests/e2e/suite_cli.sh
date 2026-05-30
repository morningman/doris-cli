# shellcheck shell=bash
# suite_cli — CLI-level contract. Pure argument parsing / help / error paths.
# These need no live cluster (clap handles --version/--help/bad-args before any
# connection). The one runtime path here — `sql` with no query — bails before
# connecting, so we run it in stateless mode to skip the auth lookup.

suite_cli() {
  suite_banner "CLI contract (offline: version / help / argument errors)"
  DCLI_STATELESS=1

  # --version / -V : clap prints "doriscli <ver>" and exits 0.
  expect_stdout_contains "cli: --version prints the binary name" "doriscli" --version
  expect_stdout_contains "cli: -V short flag works"             "doriscli" -V

  # --help lists usage and the subcommands.
  expect_stdout_contains "cli: --help shows Usage"     "Usage"  --help
  expect_stdout_contains "cli: --help lists 'sql'"     "sql"    --help
  expect_stdout_contains "cli: --help lists 'profile'" "profile" --help

  # `sql` with neither a query nor -f bails at runtime (before connecting).
  EXPECT_ERR_MATCH="Provide a SQL query"
  expect_err "cli: 'sql' with no query is rejected" sql
  EXPECT_ERR_MATCH=""

  # Unknown subcommand → clap usage error, non-zero exit.
  expect_err "cli: unknown subcommand is rejected" frobnicate-xyz

  # `tablet` requires a positional table name.
  expect_err "cli: 'tablet' with no table is rejected" tablet

  # `profile` requires a subcommand (list/get/diff/history).
  expect_err "cli: 'profile' with no action is rejected" profile

  # Unknown flag on a real subcommand → clap error.
  expect_err "cli: unknown flag is rejected" sql "SELECT 1" --definitely-not-a-flag
}
