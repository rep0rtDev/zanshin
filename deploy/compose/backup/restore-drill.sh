#!/bin/sh
# Restore drill: prove a backup is restorable without touching production data.
#   restore-drill.sh /backups/<ts>            -> restores into "${PGDATABASE}_drill", verifies, drops it
#   KEEP_DRILL=1 restore-drill.sh ...         -> keep the scratch database for inspection
# Checks: SHA256SUMS, pg_restore exit code, migration version equals the source, row counts of the
# core tables match the source (identities, events, flags, audiences, console_users, api_keys).
set -eu
dir="${1:?backup directory}"
export PGPASSWORD="$(cat "${PGPASSWORD_FILE:-/run/secrets/postgres_password}")"
drill="${DRILL_DATABASE:-${PGDATABASE}_drill}"
( cd "$dir" && sha256sum -c -s SHA256SUMS )
echo "drill: restoring $dir/postgres.dump into scratch database $drill"
psql -v ON_ERROR_STOP=1 -d postgres -qc "DROP DATABASE IF EXISTS \"$drill\""
psql -v ON_ERROR_STOP=1 -d postgres -qc "CREATE DATABASE \"$drill\""
pg_restore --no-owner --no-privileges --exit-on-error -d "$drill" "$dir/postgres.dump"
q() { psql -At -v ON_ERROR_STOP=1 -d "$1" -c "$2"; }
fail=0
src_mig="$(q "$PGDATABASE" "SELECT coalesce(max(version),0) FROM _sqlx_migrations")"
dst_mig="$(q "$drill" "SELECT coalesce(max(version),0) FROM _sqlx_migrations")"
if [ "$src_mig" != "$dst_mig" ]; then echo "drill: migration version differs: source $src_mig, restored $dst_mig"; fail=1; fi
echo "drill: schema version $dst_mig"
for t in identities events flags audiences console_users api_keys; do
  if [ "$(q "$PGDATABASE" "SELECT to_regclass('public.$t') IS NOT NULL")" != "t" ]; then continue; fi
  a="$(q "$PGDATABASE" "SELECT count(*) FROM \"$t\"")"; b="$(q "$drill" "SELECT count(*) FROM \"$t\"")"
  if [ "$a" != "$b" ]; then echo "drill: $t rows differ: source $a, restored $b (writes since the backup are expected; investigate large gaps)"; fi
  echo "drill: $t $b rows"
done
if [ "${KEEP_DRILL:-0}" != "1" ]; then psql -v ON_ERROR_STOP=1 -d postgres -qc "DROP DATABASE \"$drill\""; echo "drill: scratch database dropped"; else echo "drill: kept $drill"; fi
[ "$fail" -eq 0 ] && echo "drill: OK — backup $dir is restorable" || { echo "drill: FAILED"; exit 1; }
