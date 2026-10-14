# Nightly PostgreSQL + ClickHouse backups with local rotation and optional S3 offsite copy.
FROM alpine:3.20
RUN apk add --no-cache postgresql16-client curl zstd ca-certificates tzdata minio-client \
    && adduser -D -u 10002 backup && mkdir -p /backups && chown backup /backups
COPY backup.sh restore-pg.sh restore-ch.sh entrypoint.sh /usr/local/bin/
RUN chmod +x /usr/local/bin/*.sh
USER backup
VOLUME /backups
ENTRYPOINT ["entrypoint.sh"]
