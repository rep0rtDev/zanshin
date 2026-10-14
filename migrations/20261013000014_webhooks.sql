CREATE TABLE webhooks (
    id                uuid PRIMARY KEY,
    key               text NOT NULL UNIQUE,
    name              text NOT NULL DEFAULT '',
    url               text NOT NULL,
    secret            text NOT NULL DEFAULT '',
    events            text[] NOT NULL DEFAULT '{*}',
    enabled           boolean NOT NULL DEFAULT true,
    batch_size        integer NOT NULL DEFAULT 1,
    batch_window_secs integer NOT NULL DEFAULT 0,
    headers           jsonb NOT NULL DEFAULT '{}'::jsonb,
    max_attempts      integer NOT NULL DEFAULT 5,
    tags              text[] NOT NULL DEFAULT '{}',
    version           bigint NOT NULL DEFAULT 1,
    created_at        timestamptz NOT NULL DEFAULT now(),
    updated_at        timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE webhook_deliveries (
    id              uuid PRIMARY KEY,
    webhook_id      uuid NOT NULL REFERENCES webhooks (id) ON DELETE CASCADE,
    event_types     text[] NOT NULL DEFAULT '{}',
    payload         jsonb NOT NULL,
    status          text NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'sent', 'failed', 'dead')),
    attempts        integer NOT NULL DEFAULT 0,
    next_attempt_at timestamptz,
    last_status     integer,
    last_error      text NOT NULL DEFAULT '',
    created_at      timestamptz NOT NULL DEFAULT now(),
    delivered_at    timestamptz
);
CREATE INDEX webhook_deliveries_webhook_idx ON webhook_deliveries (webhook_id, created_at DESC);
CREATE INDEX webhook_deliveries_due_idx ON webhook_deliveries (next_attempt_at) WHERE status IN ('pending', 'failed');
