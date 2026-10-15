-- Experiments v4: mutually exclusive layers, holdout groups, guardrail metrics with auto-pause and
-- bandit (Thompson sampling) traffic allocation.

ALTER TABLE experiments
    ADD COLUMN IF NOT EXISTS layer text,
    ADD COLUMN IF NOT EXISTS holdout text,
    ADD COLUMN IF NOT EXISTS guardrails jsonb NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN IF NOT EXISTS bandit jsonb;

CREATE INDEX IF NOT EXISTS experiments_layer_idx ON experiments (layer) WHERE layer IS NOT NULL;

-- A holdout is a deterministic slice of players kept out of every experiment that references it,
-- so the long-term effect of experimentation can be measured against an untouched baseline.
CREATE TABLE IF NOT EXISTS holdouts (
    key          text PRIMARY KEY,
    name         text NOT NULL DEFAULT '',
    description  text NOT NULL DEFAULT '',
    percentage   double precision NOT NULL DEFAULT 5,
    salt         text NOT NULL,
    enabled      boolean NOT NULL DEFAULT true,
    version      bigint NOT NULL DEFAULT 1,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);
