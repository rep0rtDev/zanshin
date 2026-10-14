CREATE TABLE push_tokens (
    identity_id  uuid NOT NULL REFERENCES identities (id) ON DELETE CASCADE,
    platform     text NOT NULL CHECK (platform IN ('fcm', 'apns')),
    token        text NOT NULL,
    device_id    text NOT NULL DEFAULT '',
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    failures     integer NOT NULL DEFAULT 0,
    PRIMARY KEY (platform, token)
);
CREATE INDEX push_tokens_identity_idx ON push_tokens (identity_id);

ALTER TABLE messages ADD COLUMN push boolean NOT NULL DEFAULT false;
ALTER TABLE inbox ADD COLUMN pushed_at timestamptz;
