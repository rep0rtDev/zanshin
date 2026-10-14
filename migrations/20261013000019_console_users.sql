-- Console users with password + TOTP login, server-side console sessions, password resets, and
-- fine-grained permissions on admin API keys (plus key rotation lineage).

CREATE TABLE console_users (
    id                  uuid PRIMARY KEY,
    email               text NOT NULL UNIQUE,
    name                text NOT NULL,
    password_hash       text,
    permissions         jsonb NOT NULL DEFAULT '{"*":["read"]}',
    archetype           text,
    totp_secret         text,
    totp_enabled        boolean NOT NULL DEFAULT false,
    recovery_codes      text[] NOT NULL DEFAULT '{}',
    disabled            boolean NOT NULL DEFAULT false,
    must_change_password boolean NOT NULL DEFAULT false,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    last_login_at       timestamptz,
    password_changed_at timestamptz,
    created_by          text
);

CREATE TABLE console_user_sessions (
    id          uuid PRIMARY KEY,
    user_id     uuid NOT NULL REFERENCES console_users(id) ON DELETE CASCADE,
    token_hash  bytea NOT NULL UNIQUE,
    created_at  timestamptz NOT NULL DEFAULT now(),
    expires_at  timestamptz NOT NULL,
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    revoked_at  timestamptz,
    ip          text,
    user_agent  text
);
CREATE INDEX console_user_sessions_user_idx ON console_user_sessions (user_id, expires_at);

CREATE TABLE console_password_resets (
    token_hash  bytea PRIMARY KEY,
    user_id     uuid NOT NULL REFERENCES console_users(id) ON DELETE CASCADE,
    created_at  timestamptz NOT NULL DEFAULT now(),
    expires_at  timestamptz NOT NULL,
    used_at     timestamptz
);

ALTER TABLE api_keys
    ADD COLUMN IF NOT EXISTS permissions  jsonb,
    ADD COLUMN IF NOT EXISTS rotated_from uuid,
    ADD COLUMN IF NOT EXISTS rotated_to   uuid;
