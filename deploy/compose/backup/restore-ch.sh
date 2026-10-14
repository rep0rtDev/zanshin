#!/bin/sh
# usage: restore-ch.sh /backups/<ts>   — recreates the ClickHouse tables from the native exports.
set -eu
dir="${1:?backup directory}"
chpass="$(cat "${CH_PASSWORD_FILE:-/run/secrets/clickhouse_password}")"
q() { curl -fsS --user "$CH_USER:$chpass" "$CH_URL/?database=zanshin" --data-binary "$1"; }
( cd "$dir" && sha256sum -c --quiet SHA256SUMS )
curl -fsS --user "$CH_USER:$chpass" "$CH_URL/" --data-binary "CREATE DATABASE IF NOT EXISTS zanshin" >/dev/null
for f in "$dir"/ch.*.sql; do
  t="$(basename "$f" .sql)"; t="${t#ch.}"
  echo "table $t"
  q "DROP TABLE IF EXISTS \"$t\"" >/dev/null
  q "$(cat "$f")" >/dev/null
  zstd -dc "$dir/ch.$t.native.zst" | curl -fsS --user "$CH_USER:$chpass" "$CH_URL/?database=zanshin&query=INSERT%20INTO%20%22$t%22%20FORMAT%20Native" --data-binary @- >/dev/null
done
echo "clickhouse restored"
