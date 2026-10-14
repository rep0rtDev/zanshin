#!/bin/sh
# usage: restore-pg.sh /backups/<ts>   — restores PostgreSQL into the (empty or existing) database.
# Stop the zanshin services first: docker compose stop zanshin-1 zanshin-2
set -eu
dir="${1:?backup directory}"
export PGPASSWORD="$(cat "${PGPASSWORD_FILE:-/run/secrets/postgres_password}")"
( cd "$dir" && sha256sum -c --quiet SHA256SUMS )
echo "restoring $dir/postgres.dump into $PGDATABASE (existing objects are dropped)"
pg_restore --clean --if-exists --no-owner --no-privileges --exit-on-error -d "$PGDATABASE" "$dir/postgres.dump"
echo "postgres restored"
