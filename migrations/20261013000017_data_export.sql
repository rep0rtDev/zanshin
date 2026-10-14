-- Event export sinks, their run log, and configuration history (undo).

CREATE TABLE export_sinks (
    id            uuid PRIMARY KEY,
    key           text NOT NULL UNIQUE,
    name          text NOT NULL DEFAULT '',
    kind          text NOT NULL CHECK (kind IN ('s3', 'http', 'bigquery')),
    config        jsonb NOT NULL DEFAULT '{}'::jsonb,
    format        text NOT NULL DEFAULT 'jsonl' CHECK (format IN ('jsonl', 'csv')),
    gzip          boolean NOT NULL DEFAULT true,
    enabled       boolean NOT NULL DEFAULT true,
    events        text[] NOT NULL DEFAULT '{}',
    interval_secs bigint NOT NULL DEFAULT 300,
    batch_size    integer NOT NULL DEFAULT 10000,
    cursor_at     timestamptz,
    cursor_id     uuid,
    next_run_at   timestamptz NOT NULL DEFAULT now(),
    last_run_at   timestamptz,
    last_error    text NOT NULL DEFAULT '',
    tags          text[] NOT NULL DEFAULT '{}',
    version       bigint NOT NULL DEFAULT 1,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX export_sinks_due_idx ON export_sinks (next_run_at) WHERE enabled;

CREATE TABLE export_runs (
    id          uuid PRIMARY KEY,
    sink_id     uuid NOT NULL REFERENCES export_sinks (id) ON DELETE CASCADE,
    started_at  timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz,
    status      text NOT NULL DEFAULT 'running'
        CHECK (status IN ('running', 'ok', 'empty', 'failed')),
    rows        bigint NOT NULL DEFAULT 0,
    bytes       bigint NOT NULL DEFAULT 0,
    object      text NOT NULL DEFAULT '',
    error       text NOT NULL DEFAULT ''
);
CREATE INDEX export_runs_sink_idx ON export_runs (sink_id, started_at DESC);

CREATE TABLE config_history (
    id      uuid PRIMARY KEY,
    kind    text NOT NULL,
    key     text NOT NULL,
    version bigint NOT NULL DEFAULT 0,
    body    jsonb,
    deleted boolean NOT NULL DEFAULT false,
    actor   text NOT NULL DEFAULT '',
    at      timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX config_history_entity_idx ON config_history (kind, key, at DESC);
CREATE INDEX config_history_at_idx ON config_history (at DESC);
