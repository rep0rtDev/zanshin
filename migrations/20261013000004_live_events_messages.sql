CREATE TABLE live_events (
    id             uuid PRIMARY KEY,
    key            text NOT NULL UNIQUE,
    name           text NOT NULL DEFAULT '',
    description    text NOT NULL DEFAULT '',
    enabled        boolean NOT NULL DEFAULT true,
    condition      jsonb NOT NULL DEFAULT '{"all":[]}'::jsonb,
    audiences      text[] NOT NULL DEFAULT '{}',
    starts_at      timestamptz NOT NULL,
    ends_at        timestamptz,
    duration_secs  bigint,
    recurrence     text,
    payload        jsonb NOT NULL DEFAULT 'null'::jsonb,
    tags           text[] NOT NULL DEFAULT '{}',
    version        bigint NOT NULL DEFAULT 1,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE messages (
    id           uuid PRIMARY KEY,
    key          text NOT NULL UNIQUE,
    name         text NOT NULL DEFAULT '',
    enabled      boolean NOT NULL DEFAULT true,
    title        text NOT NULL,
    body         text NOT NULL DEFAULT '',
    image_url    text NOT NULL DEFAULT '',
    payload      jsonb NOT NULL DEFAULT 'null'::jsonb,
    condition    jsonb NOT NULL DEFAULT '{"all":[]}'::jsonb,
    audiences    text[] NOT NULL DEFAULT '{}',
    percentage   double precision NOT NULL DEFAULT 100,
    starts_at    timestamptz,
    ends_at      timestamptz,
    ttl_secs     bigint,
    priority     integer NOT NULL DEFAULT 0,
    tags         text[] NOT NULL DEFAULT '{}',
    version      bigint NOT NULL DEFAULT 1,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);

-- Delivered copies. message_id is NULL for direct (server-sent) messages.
CREATE TABLE inbox (
    id            uuid PRIMARY KEY,
    identity_id   uuid NOT NULL REFERENCES identities (id) ON DELETE CASCADE,
    message_id    uuid REFERENCES messages (id) ON DELETE CASCADE,
    key           text,
    title         text NOT NULL,
    body          text NOT NULL DEFAULT '',
    image_url     text NOT NULL DEFAULT '',
    payload       jsonb NOT NULL DEFAULT 'null'::jsonb,
    priority      integer NOT NULL DEFAULT 0,
    delivered_at  timestamptz NOT NULL DEFAULT now(),
    read_at       timestamptz,
    consumed_at   timestamptz,
    expires_at    timestamptz
);
CREATE UNIQUE INDEX inbox_identity_message_uq ON inbox (identity_id, message_id) WHERE message_id IS NOT NULL;
CREATE INDEX inbox_identity_idx ON inbox (identity_id, consumed_at, delivered_at DESC);
