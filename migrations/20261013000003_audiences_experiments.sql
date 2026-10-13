CREATE TABLE audiences (
    id           uuid PRIMARY KEY,
    key          text NOT NULL UNIQUE,
    name         text NOT NULL DEFAULT '',
    description  text NOT NULL DEFAULT '',
    condition    jsonb NOT NULL DEFAULT '{"all":[]}'::jsonb,
    percentage   double precision NOT NULL DEFAULT 100,
    enabled      boolean NOT NULL DEFAULT true,
    salt         text NOT NULL DEFAULT '',
    tags         text[] NOT NULL DEFAULT '{}',
    version      bigint NOT NULL DEFAULT 1,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE experiments (
    id           uuid PRIMARY KEY,
    key          text NOT NULL UNIQUE,
    name         text NOT NULL DEFAULT '',
    description  text NOT NULL DEFAULT '',
    hypothesis   text NOT NULL DEFAULT '',
    status       text NOT NULL CHECK (status IN ('draft', 'running', 'paused', 'stopped')),
    condition    jsonb NOT NULL DEFAULT '{"all":[]}'::jsonb,
    audiences    text[] NOT NULL DEFAULT '{}',
    percentage   double precision NOT NULL DEFAULT 100,
    variants     jsonb NOT NULL,
    control      text,
    flag_key     text,
    salt         text NOT NULL DEFAULT '',
    starts_at    timestamptz,
    ends_at      timestamptz,
    tags         text[] NOT NULL DEFAULT '{}',
    version      bigint NOT NULL DEFAULT 1,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX experiments_status_idx ON experiments (status);
CREATE INDEX experiments_flag_key_idx ON experiments (flag_key) WHERE flag_key IS NOT NULL;

-- Sticky per-identity variant assignment.
CREATE TABLE experiment_assignments (
    experiment_id  uuid NOT NULL REFERENCES experiments (id) ON DELETE CASCADE,
    identity_id    uuid NOT NULL REFERENCES identities (id) ON DELETE CASCADE,
    variant        text NOT NULL,
    assigned_at    timestamptz NOT NULL DEFAULT now(),
    exposed_at     timestamptz,
    exposures      bigint NOT NULL DEFAULT 0,
    PRIMARY KEY (experiment_id, identity_id)
);
CREATE INDEX experiment_assignments_identity_idx ON experiment_assignments (identity_id);
CREATE INDEX experiment_assignments_variant_idx ON experiment_assignments (experiment_id, variant);
