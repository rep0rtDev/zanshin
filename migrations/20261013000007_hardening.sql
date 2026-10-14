-- Hardening: API key scopes/expiry/quotas and an audit log of admin actions.

ALTER TABLE api_keys
    ADD COLUMN read_only             boolean NOT NULL DEFAULT false,
    ADD COLUMN expires_at            timestamptz,
    ADD COLUMN rate_limit_per_minute integer CHECK (rate_limit_per_minute IS NULL OR rate_limit_per_minute > 0);

CREATE TABLE audit_log (
    id           uuid PRIMARY KEY,
    at           timestamptz NOT NULL DEFAULT now(),
    actor_key_id uuid,
    actor_name   text NOT NULL,
    actor_kind   text NOT NULL,
    method       text NOT NULL,
    path         text NOT NULL,
    status       integer NOT NULL,
    request_id   text,
    ip           text,
    body         jsonb
);

CREATE INDEX audit_log_at_idx ON audit_log (at DESC);
CREATE INDEX audit_log_actor_idx ON audit_log (actor_name, at DESC);
CREATE INDEX audit_log_path_idx ON audit_log (path text_pattern_ops);
