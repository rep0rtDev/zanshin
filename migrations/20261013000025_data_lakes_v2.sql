-- Data lakes v2: notification channels (Slack / Discord / webhook) for config changes,
-- failed export runs, unhealthy integrations and alerts.

CREATE TABLE IF NOT EXISTS notification_channels (
    id            uuid PRIMARY KEY,
    key           text NOT NULL UNIQUE,
    name          text NOT NULL DEFAULT '',
    kind          text NOT NULL,
    url           text NOT NULL,
    topics        text[] NOT NULL DEFAULT '{}',
    kinds         text[] NOT NULL DEFAULT '{}',
    enabled       boolean NOT NULL DEFAULT true,
    headers       jsonb NOT NULL DEFAULT '{}'::jsonb,
    last_status   integer,
    last_error    text NOT NULL DEFAULT '',
    last_sent_at  timestamptz,
    tags          text[] NOT NULL DEFAULT '{}',
    version       bigint NOT NULL DEFAULT 1,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now()
);

-- New sink kinds and the Parquet format.
ALTER TABLE export_sinks DROP CONSTRAINT IF EXISTS export_sinks_kind_check;
ALTER TABLE export_sinks ADD CONSTRAINT export_sinks_kind_check
    CHECK (kind IN ('s3', 'http', 'bigquery', 'snowflake', 'redshift'));
ALTER TABLE export_sinks DROP CONSTRAINT IF EXISTS export_sinks_format_check;
ALTER TABLE export_sinks ADD CONSTRAINT export_sinks_format_check
    CHECK (format IN ('jsonl', 'csv', 'parquet'));
