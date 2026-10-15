# Nightly PostgreSQL + ClickHouse backups with local rotation and optional S3 offsite copy.
FROM alpine:3.20
RUN apk add --no-cache postgresql16-client curl zstd ca-certificates tzdata aws-cli \
    && adduser -D -u 10002 backup && mkdir -p /backups && chown backup /backups
COPY backup.sh restore-pg.sh restore-ch.sh restore-drill.sh entrypoint.sh /usr/local/bin/
RUN chmod +x /usr/local/bin/*.sh
USER backup
VOLUME /backups
ENTRYPOINT ["entrypoint.sh"]
