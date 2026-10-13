-- Identities --------------------------------------------------------------
CREATE TABLE identities (
    id                  uuid PRIMARY KEY,
    external_id         text NOT NULL UNIQUE,
    default_properties  jsonb NOT NULL DEFAULT '{}'::jsonb,
    computed_properties jsonb NOT NULL DEFAULT '{}'::jsonb,
    custom_properties   jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    last_seen_at        timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX identities_last_seen_idx ON identities (last_seen_at);
CREATE INDEX identities_default_props_gin ON identities USING gin (default_properties jsonb_path_ops);
CREATE INDEX identities_custom_props_gin ON identities USING gin (custom_properties jsonb_path_ops);

-- Sessions ----------------------------------------------------------------
CREATE TABLE sessions (
    id            uuid PRIMARY KEY,
    identity_id   uuid NOT NULL REFERENCES identities (id) ON DELETE CASCADE,
    refresh_hash  bytea NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    rotated_at    timestamptz NOT NULL DEFAULT now(),
    expires_at    timestamptz NOT NULL,
    revoked_at    timestamptz
);
CREATE INDEX sessions_identity_idx ON sessions (identity_id);
CREATE INDEX sessions_expires_idx ON sessions (expires_at);

-- API keys ----------------------------------------------------------------
CREATE TABLE api_keys (
    id            uuid PRIMARY KEY,
    name          text NOT NULL,
    kind          text NOT NULL CHECK (kind IN ('client', 'server', 'admin')),
    prefix        text NOT NULL,
    key_hash      bytea NOT NULL UNIQUE,
    created_at    timestamptz NOT NULL DEFAULT now(),
    last_used_at  timestamptz,
    revoked_at    timestamptz
);
