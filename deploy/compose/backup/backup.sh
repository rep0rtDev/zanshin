#!/bin/sh
# Dumps PostgreSQL (pg_dump custom format) and ClickHouse (native BACKUP to the server's backups
# path, then streamed out as a tar) into /backups/<utc-timestamp>/, rotates by BACKUP_KEEP_DAYS and
# mirrors to S3 when S3_URL is set. Safe to run while the stack is serving traffic.
set -eu
ts="$(date -u +%Y%m%dT%H%M%SZ)"
dir="/backups/$ts"
mkdir -p "$dir"
export PGPASSWORD="$(cat "${PGPASSWORD_FILE:-/run/secrets/postgres_password}")"
chpass="$(cat "${CH_PASSWORD_FILE:-/run/secrets/clickhouse_password}")"

echo "[$ts] postgres: pg_dump $PGDATABASE"
pg_dump --format=custom --compress=zstd:6 --no-owner --no-privileges -f "$dir/postgres.dump" "$PGDATABASE"
pg_dump --schema-only --no-owner -f "$dir/postgres.schema.sql" "$PGDATABASE"

echo "[$ts] clickhouse: BACKUP DATABASE zanshin"
# Native backup keeps parts compressed; the file lands inside the ClickHouse container volume.
curl -fsS --user "$CH_USER:$chpass" "$CH_URL/" \
  --data-binary "BACKUP DATABASE zanshin TO File('/var/lib/clickhouse/backups/$ts.zip') SETTINGS compression_method='zstd', compression_level=3" >/dev/null
# Export every table as native blocks too, so a restore does not depend on the volume surviving.
tables="$(curl -fsS --user "$CH_USER:$chpass" "$CH_URL/" --data-binary "SELECT name FROM system.tables WHERE database='zanshin' AND engine NOT LIKE '%View%' FORMAT TSV")"
for t in $tables; do
  curl -fsS --user "$CH_USER:$chpass" "$CH_URL/?database=zanshin" --data-binary "SELECT * FROM \"$t\" FORMAT Native" | zstd -q -3 -o "$dir/ch.$t.native.zst"
  curl -fsS --user "$CH_USER:$chpass" "$CH_URL/?database=zanshin" --data-binary "SHOW CREATE TABLE \"$t\" FORMAT TSVRaw" > "$dir/ch.$t.sql"
done

( cd "$dir" && sha256sum -- * > SHA256SUMS )
echo "[$ts] done: $(du -sh "$dir" | cut -f1)"

keep="${BACKUP_KEEP_DAYS:-14}"
find /backups -mindepth 1 -maxdepth 1 -type d -mtime +"$keep" -exec rm -rf {} + 2>/dev/null || true
curl -fsS --user "$CH_USER:$chpass" "$CH_URL/" --data-binary "SELECT name FROM system.backups FORMAT TSV" >/dev/null 2>&1 || true

if [ -n "${S3_URL:-}" ]; then
  secret="$(cat "${S3_SECRET_KEY_FILE:-/run/secrets/backup_s3_secret}")"
  mc alias set offsite "${S3_URL%/*}" "$S3_ACCESS_KEY" "$secret" >/dev/null
  mc mirror --quiet --retry "$dir" "offsite/${S3_URL##*/}/$ts"
  echo "[$ts] mirrored to $S3_URL/$ts"
fi
