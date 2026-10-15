#!/usr/bin/env sh
# Generates the secret files docker-compose.yml expects. Existing files are kept.
set -eu
cd "$(dirname "$0")/secrets"
gen() { [ -s "$1" ] && echo "keep  $1" || { openssl rand -base64 "${2:-48}" | tr -d '\n=/+' | cut -c1-"${3:-48}" > "$1"; chmod 600 "$1"; echo "wrote $1"; }; }
gen postgres_password 36 32
gen clickhouse_password 36 32
gen jwt_secret 96 64
gen admin_bootstrap_key 48 40
[ -s data_keys ] && echo "keep  data_keys" || { printf 'k1:%s' "$(openssl rand -base64 32)" > data_keys; chmod 600 data_keys; echo "wrote data_keys"; }
gen grafana_admin_password 24 20
[ -s database_url ] && echo "keep  database_url" || { printf 'postgres://zanshin:%s@postgres:5432/zanshin?sslmode=disable' "$(cat postgres_password)" > database_url; chmod 600 database_url; echo "wrote database_url"; }
[ -e backup_s3_secret ] || { : > backup_s3_secret; chmod 600 backup_s3_secret; echo "empty backup_s3_secret (fill in for offsite backups)"; }
echo; echo "admin bootstrap key (log in to /console with it, then create personal keys):"; cat admin_bootstrap_key; echo
