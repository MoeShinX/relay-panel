#!/usr/bin/env bash
# Check that sqlite_repo/tests.rs and pg_repo/tests.rs have equivalent test
# function names (after normalising the `pg_` prefix on PG tests) modulo an
# explicit allowlist.
#
# Non-zero exit = drift detected. CI and refactor-check.sh call this script
# to prevent single-backend test additions from going unnoticed.
#
# Usage:
#   scripts/check-repo-test-parity.sh          # check both
#   scripts/check-repo-test-parity.sh -v       # verbose (print all names)

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

SQLITE_FILE="$ROOT_DIR/crates/panel/src/db/sqlite_repo/tests.rs"
PG_FILE="$ROOT_DIR/crates/panel/src/db/pg_repo/tests.rs"
ALLOWLIST_FILE="$ROOT_DIR/scripts/repo-test-parity-allowlist.txt"

verbose=0
if [[ "${1:-}" == "-v" || "${1:-}" == "--verbose" ]]; then
  verbose=1
fi

# ── Parse test function names from a Rust test file ──
# Matches lines like: async fn test_name() {  or  async fn test_name(suffix: ...) {
# Excludes helper functions (repo, cleanup, seed_*, pg_url, replace_db_in_url).
#
# NOTE: exclusion runs BEFORE the pg_ prefix is stripped (see pg_names below),
# so a PG-side helper has to be listed under its REAL name (pg_seed_code), not
# the normalised one. List only the normalised name and the helper survives
# here, gets normalised to seed_code, and is then compared against a SQLite
# helper this list already removed — reported as bogus drift.
extract_tests() {
  local file="$1"
  grep -oP '^\s*async fn\s+\K[a-z_][a-z0-9_]*(?=\s*\()' "$file" \
	    | grep -vE '^(repo|cleanup|seed_group|seed_group_typed|seed_user|pg_seed_user|seed_code|pg_seed_code|balance_of|pg_balance_of|pg_url|replace_db_in_url|placeholders|_placeholders_unused)$' \
    || true
}

# ── Load allowlist ──
load_allowlist() {
  if [[ -f "$ALLOWLIST_FILE" ]]; then
    grep -oP '^[a-z_][a-z0-9_]*' "$ALLOWLIST_FILE" | grep -v '^#' || true
  fi
}

sqlite_names=$(extract_tests "$SQLITE_FILE" | sort -u)
pg_raw=$(extract_tests "$PG_FILE" | sort -u)
allowlist=$(load_allowlist | sort -u)

# Normalise PG names: drop `pg_` prefix.
pg_names=$(echo "$pg_raw" | sed 's/^pg_//' | sort -u)

if [[ "$verbose" -eq 1 ]]; then
  echo "=== SQLite tests ($(echo "$sqlite_names" | grep -c .)) ==="
  echo "$sqlite_names"
  echo ""
  echo "=== PG tests ($(echo "$pg_raw" | grep -c .)) ==="
  echo "$pg_raw"
  echo ""
  echo "=== Allowlist ($(echo "$allowlist" | grep -c .)) ==="
  echo "$allowlist"
  echo ""
fi

# ── Compute diffs ──

# Only in SQLite (not in PG normalised)
only_sqlite=$(comm -23 <(echo "$sqlite_names") <(echo "$pg_names"))
# Only in PG (not in SQLite)
only_pg=$(comm -13 <(echo "$sqlite_names") <(echo "$pg_names"))

# Apply allowlist: remove allowed names from the diff sets.
only_sqlite_filtered=$(comm -23 <(echo "$only_sqlite" | sort) <(echo "$allowlist"))
only_pg_filtered=$(comm -23 <(echo "$only_pg" | sort) <(echo "$allowlist"))

# ── Report ──

exit_code=0

if [[ -n "$only_sqlite_filtered" ]]; then
  echo "ERROR: Tests only in sqlite_repo (not in pg_repo):" >&2
  echo "$only_sqlite_filtered" | while read -r name; do
    echo "  - $name (missing pg_$name)" >&2
  done
  exit_code=1
fi

if [[ -n "$only_pg_filtered" ]]; then
  echo "ERROR: Tests only in pg_repo (not in sqlite_repo):" >&2
  echo "$only_pg_filtered" | while read -r name; do
    echo "  + $name (pg_$name present, missing in sqlite)" >&2
  done
  exit_code=1
fi

if [[ "$exit_code" -eq 0 ]]; then
  sqlite_count=$(echo "$sqlite_names" | grep -c . || echo 0)
  pg_count=$(echo "$pg_raw" | grep -c . || echo 0)
  echo "OK: sqlite_repo ($sqlite_count tests) and pg_repo ($pg_count tests) are in parity."
  echo "    Allowlist entries: $(echo "$allowlist" | grep -c . || echo 0)"
fi

exit $exit_code
