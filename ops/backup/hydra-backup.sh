#!/usr/bin/env bash
# Nightly Hydra backup: pg_dump -Fc of the database, pg_dumpall --globals-only (roles), and a
# sidecar of row counts for hydra-restore-test.sh. See README.md.
#
# usage: hydra-backup.sh [TARGET_DIR]
# env:   HYDRA_BACKUP_DIR        target directory (argument wins); put it on ANOTHER machine
#        HYDRA_BACKUP_KEEP_DAYS  delete backups older than this (default 30)
#        PGHOST PGPORT PGUSER PGDATABASE (default hydra), PGPASSFILE: standard libpq settings.
#        The user must be a superuser (pg_dumpall --globals-only reads role passwords).
set -euo pipefail
umask 077 # dumps hold role password hashes: owner-only, whatever the target filesystem defaults to

dir="${1:-${HYDRA_BACKUP_DIR:-}}"
keep="${HYDRA_BACKUP_KEEP_DAYS:-30}"
export PGDATABASE="${PGDATABASE:-hydra}"
# Keep in step with hydra-backup.ps1 and hydra-restore-test.*.
tables="_sqlx_migrations manufacturers symbol_defs assets valve_types parts blocks part_assets catalogue_proposals circuits circuit_revisions"

stamp="$(date -u +%Y%m%dT%H%M%SZ)"
partials=()
fail() { echo "hydra-backup: FAILED: $*" >&2; exit 1; }
cleanup() {
  rc=$?
  [ ${#partials[@]} -eq 0 ] || rm -f "${partials[@]}"
  [ "$rc" -eq 0 ] || echo "hydra-backup: FAILED (exit $rc); nothing from this run was kept" >&2
}
trap cleanup EXIT

[ -n "$dir" ] || fail "no target directory: pass it as an argument or set HYDRA_BACKUP_DIR"
case "$keep" in '' | *[!0-9]*) fail "HYDRA_BACKUP_KEEP_DAYS must be a number, got '$keep'" ;; esac
[ -d "$dir" ] && [ -w "$dir" ] || fail "target directory '$dir' is missing or not writable (is the share mounted?)"
for tool in pg_dump pg_dumpall pg_restore psql; do
  command -v "$tool" >/dev/null || fail "$tool not found in PATH"
done

# A dump from a newer pg_dump can contain settings the server rejects on restore (e.g.
# transaction_timeout from 17+ into 15), so the client major must equal the server major.
server_major="$(psql -X -qAt -c 'SHOW server_version_num' | cut -c1-2)" || fail "cannot connect to '$PGDATABASE'"
client_major="$(pg_dump --version | sed -E 's/[^0-9]*([0-9]+).*/\1/')"
[ "$server_major" = "$client_major" ] || fail "pg_dump is version $client_major but the server is $server_major; use matching client tools"

counts() { # prints "table count" lines
  for t in $tables; do
    echo "$t $(psql -X -qAt -v ON_ERROR_STOP=1 -c "SELECT count(*) FROM $t")" || return 1
  done
}

dump="$dir/hydra-$stamp.dump"
globals="$dir/hydra-globals-$stamp.sql"
sidecar="$dir/hydra-$stamp.counts"
partials=("$dump.partial" "$globals.partial" "$sidecar.partial")

before="$(counts)" || fail "could not count rows in '$PGDATABASE' (connection or schema problem)"
pg_dump -Fc -f "$dump.partial" || fail "pg_dump of '$PGDATABASE' failed (client major version must be >= server)"
pg_restore --list "$dump.partial" >/dev/null || fail "the dump is unreadable"
after="$(counts)" || fail "could not count rows after the dump"
pg_dumpall --globals-only -f "$globals.partial" || fail "pg_dumpall --globals-only failed (needs a superuser)"
# The dump holds between "before" and "after" rows per table. Only part_assets also loses rows
# (replacing a 3D model deletes them), so the restore test does not range-check it.
paste -d' ' <(echo "$before") <(echo "$after" | cut -d' ' -f2) >"$sidecar.partial"

# Rename the dump last: if it exists under its final name, its siblings do too.
mv "$globals.partial" "$globals"
mv "$sidecar.partial" "$sidecar"
mv "$dump.partial" "$dump"
partials=()

# Prune only after a successful backup, so a failing night never eats the history.
find "$dir" -maxdepth 1 -type f \( -name 'hydra-[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]T[0-9][0-9][0-9][0-9][0-9][0-9]Z.dump' -o -name 'hydra-[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]T[0-9][0-9][0-9][0-9][0-9][0-9]Z.counts' -o -name 'hydra-globals-[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]T[0-9][0-9][0-9][0-9][0-9][0-9]Z.sql' \) -mtime "+$keep" -delete
find "$dir" -maxdepth 1 -type f -name 'hydra-*.partial' -mtime +1 -delete
echo "hydra-backup: OK $dump ($(wc -c <"$dump") bytes)"
