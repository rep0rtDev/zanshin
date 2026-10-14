CREATE TABLE message_templates (
    id             uuid PRIMARY KEY,
    key            text NOT NULL UNIQUE,
    name           text NOT NULL DEFAULT '',
    description    text NOT NULL DEFAULT '',
    format         text NOT NULL DEFAULT 'plaintext'
        CHECK (format IN ('plaintext', 'html', 'json')),
    default_locale text NOT NULL DEFAULT 'en',
    translations   jsonb NOT NULL DEFAULT '{}'::jsonb,
    image_url      text NOT NULL DEFAULT '',
    payload        jsonb NOT NULL DEFAULT 'null'::jsonb,
    tags           text[] NOT NULL DEFAULT '{}',
    version        bigint NOT NULL DEFAULT 1,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE integrations (
    id         uuid PRIMARY KEY,
    key        text NOT NULL UNIQUE,
    name       text NOT NULL DEFAULT '',
    kind       text NOT NULL
        CHECK (kind IN ('webhook', 'onesignal', 'fcm', 'apns', 'facebook')),
    config     jsonb NOT NULL DEFAULT '{}'::jsonb,
    enabled    boolean NOT NULL DEFAULT true,
    version    bigint NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE messages
    ADD COLUMN template_key text,
    ADD COLUMN schedule     jsonb NOT NULL DEFAULT '{"kind":"once"}'::jsonb,
    ADD COLUMN channels     text[] NOT NULL DEFAULT '{inbox}';

ALTER TABLE inbox ADD COLUMN occurrence timestamptz;
DROP INDEX inbox_identity_message_uq;
CREATE UNIQUE INDEX inbox_identity_message_uq
    ON inbox (identity_id, message_id, COALESCE(occurrence, 'epoch'::timestamptz))
    WHERE message_id IS NOT NULL;

CREATE TABLE message_deliveries (
    id          uuid PRIMARY KEY,
    message_id  uuid NOT NULL REFERENCES messages (id) ON DELETE CASCADE,
    identity_id uuid NOT NULL REFERENCES identities (id) ON DELETE CASCADE,
    occurrence  timestamptz,
    channel     text NOT NULL,
    status      text NOT NULL CHECK (status IN ('sent', 'failed', 'skipped')),
    detail      text NOT NULL DEFAULT '',
    at          timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX message_deliveries_message_idx ON message_deliveries (message_id, at DESC);
