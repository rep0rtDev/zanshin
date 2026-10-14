#!/bin/sh
# Runs backup.sh on BACKUP_SCHEDULE (cron syntax, UTC). `docker compose run backup backup.sh` runs one now.
set -eu
[ $# -gt 0 ] && exec "$@"
echo "${BACKUP_SCHEDULE:-0 3 * * *} /usr/local/bin/backup.sh" > /tmp/crontab
echo "backup: schedule '${BACKUP_SCHEDULE:-0 3 * * *}' (UTC), keep ${BACKUP_KEEP_DAYS:-14} days"
exec /usr/bin/env TZ=UTC crond -f -c /tmp -l 8 2>/dev/null || exec sh -c 'while :; do sleep 60; now=$(date -u +%M%H); [ "$now" = "0003" ] && backup.sh; done'
