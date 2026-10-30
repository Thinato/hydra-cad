#!/usr/bin/env bash
# Restore a Hydra backup into a scratch database and verify it. Exits non-zero on any problem.
#
# usage: hydra-restore-test.sh [--dir DIR] [--dump FILE] [--target DB] [--globals] [--keep]
#   --dir DIR     backup directory; newest timestamped hydra-<stamp>.dump is used (default $HYDRA_BACKUP_DIR)
#   --dump FILE   restore this dump instead (its .counts / globals siblings are used if present)
#   --target DB   scratch database name (default hydra_restore_test); refused if it equals the
#                 live database name ($PGDATABASE, default hydra)
#   --globals     first apply the globals file (roles). Roles are cluster-wide: only use this on a
#                 scratch cluster, never on the live server.
#   --keep        do not drop the scratch database afterwards
# env:   HYDRA_ADMIN  path to the hydra-admin binary (required)
#        PGHOST PGPORT PGUSER PGPASSFILE: connect as a user allowed to CREATE/DROP DATABASE.
set -euo pipefail

live="${PGDATABASE:-hydra}"
target=hydra_restore_test
dir="${HYDRA_BACKUP_DIR:-}"
dump=""
use_globals=0
keep=0
fail() { echo "hydra-restore-test: FAILED: $*" >&2; exit 1; }
while [ $# -gt 0 ]; do
  case "$1" in
    --dir) dir="${2:?--dir needs a value}"; shift 2 ;;
    --dump) dump="${2:?--dump needs a value}"; shift 2 ;;
    --target) target="${2:?--target needs a value}"; shift 2 ;;
    --globals) use_globals=1; shift ;;
    --keep) keep=1; shift ;;
    *) fail "unknown argument '$1'" ;;
  esac
done

[ "$target" != "$live" ] && [ "$target" != hydra ] && [ "$target" != hydra_app ] ||
  fail "refusing to restore into '$target': it is (or looks like) the live database"
case "$target" in *[!A-Za-z0-9_]* | '') fail "target name must be letters, digits, underscore" ;; esac
[ -n "${HYDRA_ADMIN:-}" ] && [ -x "$HYDRA_ADMIN" ] || fail "set HYDRA_ADMIN to the hydra-admin binary"
if [ -z "$dump" ]; then
  [ -d "$dir" ] || fail "no --dump and no readable backup directory (--dir / HYDRA_BACKUP_DIR)"
  dump="$(find "$dir" -maxdepth 1 -type f -name 'hydra-[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]T[0-9][0-9][0-9][0-9][0-9][0-9]Z.dump' | sort | tail -n 1)"
  [ -n "$dump" ] || fail "no hydra-<stamp>.dump in '$dir'"
fi
[ -s "$dump" ] || fail "dump '$dump' is missing or empty"
stem="${dump%.dump}"
sidecar="$stem.counts"
globals="$(dirname "$dump")/hydra-globals-$(basename "$stem" | sed 's/^hydra-//').sql"
[ -f "$sidecar" ] || fail "no row-count sidecar '$sidecar'"
echo "hydra-restore-test: restoring $dump into '$target'"

# </dev/null: psql must not swallow the sidecar read by the count loop
psql_admin() { psql -X -qAt -v ON_ERROR_STOP=1 -d postgres "$@" </dev/null; }
psql_target() { psql -X -qAt -v ON_ERROR_STOP=1 -d "$target" "$@" </dev/null; }
created=0 # only ever drop a database this run created
cleanup() {
  rc=$?
  if [ "$keep" -eq 0 ] && [ "$created" -eq 1 ]; then
    psql_admin -c "DROP DATABASE IF EXISTS $target WITH (FORCE)" >/dev/null 2>&1 || echo "hydra-restore-test: could not drop '$target'" >&2
  fi
  [ "$rc" -eq 0 ] || echo "hydra-restore-test: FAILED (exit $rc)" >&2
}
trap cleanup EXIT

exists="$(psql_admin -c "SELECT 1 FROM pg_database WHERE datname = '$target'")"
[ -z "$exists" ] || fail "database '$target' already exists; drop it or pick another --target"
if [ "$use_globals" -eq 1 ]; then
  [ -f "$globals" ] || fail "no globals file '$globals'"
  psql_admin -f "$globals" >/dev/null || fail "applying globals failed"
fi
psql_admin -c "CREATE DATABASE $target TEMPLATE template0" >/dev/null || fail "cannot create '$target'"
created=1

opts=(--exit-on-error)
[ "$use_globals" -eq 1 ] || opts+=(--no-owner --no-privileges) # roles are not present on a scratch run
pg_restore "${opts[@]}" -d "$target" "$dump" || fail "pg_restore reported errors"

DATABASE_URL="postgres:///$target" "$HYDRA_ADMIN" check || fail "hydra-admin check failed on the restored database"

bad=0
while read -r table lo hi; do
  n="$(psql_target -c "SELECT count(*) FROM $table")" || fail "cannot count $table"
  if [ "$table" = part_assets ]; then # the app deletes these when a model is replaced, so no range
    echo "  $table: $n rows (not range-checked)"
    continue
  fi
  if [ "$n" -lt "$lo" ] || [ "$n" -gt "$hi" ]; then
    echo "hydra-restore-test: $table has $n rows, backup recorded $lo..$hi" >&2
    bad=1
  else
    echo "  $table: $n rows (recorded $lo..$hi)"
  fi
done <"$sidecar"
[ "$bad" -eq 0 ] || fail "row counts do not match the backup"

# Triggers must have come back and still fire, not just exist.
# The expected set is whatever the dump itself lists, so new migrations need no edit here.
expected="$(pg_restore --list "$dump" | grep -c ' TRIGGER ' || true)"
triggers="$(psql_target -c "SELECT count(*) FROM pg_trigger WHERE NOT tgisinternal")"
[ "$expected" -gt 0 ] && [ "$triggers" = "$expected" ] || fail "dump lists $expected triggers, restored database has $triggers"
[ "$(psql_target -c "SELECT count(*) FROM pg_trigger WHERE tgname = 'symbol_defs_immutable'")" = 1 ] || fail "symbol_defs_immutable trigger is missing"
if [ "$(psql_target -c "SELECT count(*) FROM symbol_defs")" -gt 0 ]; then
  # An UPDATE only the trigger can refuse (a DELETE would also fail on the valve_types foreign key).
  if err="$(psql_target -c "UPDATE symbol_defs SET key = key" 2>&1)"; then
    fail "symbol_defs rows can be updated: immutability trigger does not fire"
  fi
  case "$err" in *"never edited in place"*) ;; *) fail "symbol_defs update was refused, but not by the immutability trigger: $err" ;; esac
fi

if [ "$keep" -eq 1 ]; then
  echo "hydra-restore-test: OK (kept database '$target')"
else
  psql_admin -c "DROP DATABASE $target WITH (FORCE)" >/dev/null || fail "could not drop '$target'"
  created=0
  echo "hydra-restore-test: OK (dropped '$target')"
fi
