CREATE TABLE flags (
    id             uuid PRIMARY KEY,
    key            text NOT NULL UNIQUE,
    name           text NOT NULL DEFAULT '',
    description    text NOT NULL DEFAULT '',
    value_type     text NOT NULL CHECK (value_type IN ('bool', 'string', 'number', 'json')),
    default_value  jsonb NOT NULL,
    enabled        boolean NOT NULL DEFAULT true,
    rules          jsonb NOT NULL DEFAULT '[]'::jsonb,
    salt           text NOT NULL DEFAULT '',
    tags           text[] NOT NULL DEFAULT '{}',
    version        bigint NOT NULL DEFAULT 1,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX flags_tags_idx ON flags USING gin (tags);

-- Per-identity pinned values (QA, support, VIP handling).
CREATE TABLE flag_overrides (
    flag_id      uuid NOT NULL REFERENCES flags (id) ON DELETE CASCADE,
    identity_id  uuid NOT NULL REFERENCES identities (id) ON DELETE CASCADE,
    value        jsonb NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (flag_id, identity_id)
);
CREATE INDEX flag_overrides_identity_idx ON flag_overrides (identity_id);
