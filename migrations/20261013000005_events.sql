-- Analytics events when ClickHouse is disabled (and the ingest spool is never used otherwise).
CREATE TABLE events (
    id           uuid PRIMARY KEY,
    event_time   timestamptz NOT NULL,
    received_at  timestamptz NOT NULL DEFAULT now(),
    identity_id  uuid NOT NULL,
    external_id  text NOT NULL,
    name         text NOT NULL,
    value        text NOT NULL DEFAULT '',
    metadata     jsonb NOT NULL DEFAULT 'null'::jsonb,
    session_id   uuid,
    experiments  jsonb NOT NULL DEFAULT '{}'::jsonb
);
CREATE INDEX events_name_time_idx ON events (name, event_time DESC);
CREATE INDEX events_identity_idx ON events (identity_id, event_time DESC);
CREATE INDEX events_time_idx ON events (event_time DESC);
