# shellcheck shell=bash
# suite_auth — connection management via the ~/.doris config files (file mode),
# plus the stateless (env-var only) mode and its "never touches disk" guarantee.
#
# File-mode runs use an ISOLATED $HOME, so the real ~/.doris is never touched.
# `auth status` ALWAYS exits 0 in doriscli — so the real connectivity assertion
# is `.mysql_status == "connected"`, not the exit code.

suite_auth() {
  suite_banner "auth / use  (config-file mode + stateless mode)"

  # Start from an empty config so the empty-list assertion is meaningful.
  rm -rf "$ISOLATED_HOME/.doris"
  DCLI_STATELESS=0

  # Empty config: object with an empty environments array + a hint message.
  expect_json "auth: list is empty on a fresh config" \
    '.environments == [] and (.message|type=="string")' \
    auth list

  # Add the cluster under test as 'selftest' (becomes the default env).
  expect_json "auth: add saves an environment" \
    '.status=="added" and .name=="selftest" and (.http_probe|type=="object")' \
    auth add selftest --host "$CFG_HOST" --port "$CFG_PORT" \
      --http-port "$CFG_HTTP_PORT" --user "$CFG_USER" --password "$CFG_PASSWORD"

  # List now shows it, marked default.
  expect_json "auth: list shows the added env as default" \
    '(type=="array") and (.[0].name=="selftest") and (.[0].default==true)' \
    auth list

  # status actually connects over MySQL — this is the live connectivity gate.
  expect_json "auth: status connects to the cluster (mysql_status=connected)" \
    '.environment=="selftest" and .mysql_status=="connected"' \
    auth status
  # Surface the version we saw, for the log.
  DCLI_STATELESS=0 _run_dcli --format json auth status
  log "  ${C_DIM}cluster: version=$(jget '.doris_version')  http=$(jget '.http_status')  backends=$(jget '.backends|length')${C_RESET}"

  # `use` with no arg reports the current default.
  expect_json "use: shows current environment" \
    '.current=="selftest"' \
    use

  # mysql:// URI form — only when the password is URI-safe (parser splits on : / @).
  case "$CFG_PASSWORD" in
    *:*|*@*|*/*)
      skip "auth: add via mysql:// URI" "password contains : / or @ (not URI-safe to test)";;
    *)
      expect_json "auth: add via mysql:// URI parses host+port" \
        '.host=="'"$CFG_HOST"'" and .mysql_port=='"$CFG_PORT" \
        auth add uritest --mysql "mysql://$CFG_USER:$CFG_PASSWORD@$CFG_HOST:$CFG_PORT"

      # Switch default to it, then remove it, and confirm it's gone.
      expect_json "use: switch to another environment" \
        '.status=="switched" and .environment=="uritest"' \
        use uritest
      expect_json "auth: remove deletes an environment" \
        '.status=="removed" and .name=="uritest"' \
        auth remove uritest
      expect_json "auth: removed env no longer listed" \
        'any(.[]?; .name=="uritest") | not' \
        auth list
      # restore default
      DCLI_STATELESS=0 _run_dcli --format json use selftest
      ;;
  esac

  # ---- stateless mode (DORIS_HOST + DORIS_USER drive everything) ----
  DCLI_STATELESS=1

  # Mutating config is refused in stateless mode (multi-tenant bastion safety).
  EXPECT_ERR_MATCH="stateless"
  expect_err "stateless: 'auth add' is refused" \
    auth add nope --host "$CFG_HOST" --user "$CFG_USER"
  EXPECT_ERR_MATCH=""

  # ...but read/connect still works, purely from env vars.
  expect_json "stateless: status connects from env vars only" \
    '.mysql_status=="connected"' \
    auth status

  # And it must NOT create any config files. Prove it with a pristine HOME.
  local clean_home; clean_home="$WORKDIR/clean_home_$$"
  local saved_home="$ISOLATED_HOME"
  mkdir -p "$clean_home"
  ISOLATED_HOME="$clean_home"
  DCLI_STATELESS=1 _run_dcli --format json auth status
  if [ -e "$clean_home/.doris" ]; then
    record_fail "stateless: writes no files to disk" "created $clean_home/.doris"
  else
    record_pass "stateless: writes no files to disk"
  fi
  ISOLATED_HOME="$saved_home"
  rm -rf "$clean_home"
}
