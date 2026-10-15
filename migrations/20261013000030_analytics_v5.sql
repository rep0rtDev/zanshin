-- Analytics v5: saved SQL queries, scheduled reports, metric alerts.
CREATE TABLE IF NOT EXISTS saved_queries (
    id          uuid PRIMARY KEY,
    key         text NOT NULL UNIQUE,
    name        text NOT NULL DEFAULT '',
    description text NOT NULL DEFAULT '',
    sql         text NOT NULL,
    tags        jsonb NOT NULL DEFAULT '[]'::jsonb,
    version     bigint NOT NULL DEFAULT 1,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS report_schedules (
    id           uuid PRIMARY KEY,
    key          text NOT NULL UNIQUE,
    name         text NOT NULL DEFAULT '',
    enabled      boolean NOT NULL DEFAULT true,
    query_key    text NOT NULL,
    cron         text NOT NULL,
    timezone     text NOT NULL DEFAULT 'UTC',
    channels     jsonb NOT NULL DEFAULT '[]'::jsonb,
    email        jsonb,
    max_rows     bigint NOT NULL DEFAULT 20,
    last_run_at  timestamptz,
    last_status  text NOT NULL DEFAULT '',
    version      bigint NOT NULL DEFAULT 1,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS metric_alerts (
    id              uuid PRIMARY KEY,
    key             text NOT NULL UNIQUE,
    name            text NOT NULL DEFAULT '',
    enabled         boolean NOT NULL DEFAULT true,
    event           text NOT NULL,
    aggregation     text NOT NULL DEFAULT 'count',
    field           text NOT NULL DEFAULT '',
    condition       jsonb NOT NULL,
    window_secs     bigint NOT NULL,
    cooldown_secs   bigint NOT NULL DEFAULT 3600,
    last_value      double precision,
    last_checked_at timestamptz,
    last_fired_at   timestamptz,
    version         bigint NOT NULL DEFAULT 1,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS metric_alert_fires (
    id        uuid PRIMARY KEY,
    alert_id  uuid NOT NULL REFERENCES metric_alerts (id) ON DELETE CASCADE,
    value     double precision NOT NULL,
    baseline  double precision,
    deviation double precision,
    text      text NOT NULL DEFAULT '',
    fired_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS metric_alert_fires_alert_idx ON metric_alert_fires (alert_id, fired_at DESC);
