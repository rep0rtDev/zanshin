CREATE TABLE journeys (
    id            uuid PRIMARY KEY,
    key           text NOT NULL UNIQUE,
    name          text NOT NULL DEFAULT '',
    description   text NOT NULL DEFAULT '',
    enabled       boolean NOT NULL DEFAULT true,
    trigger       jsonb NOT NULL DEFAULT '{"kind":"manual"}'::jsonb,
    condition     jsonb NOT NULL DEFAULT '{"all":[]}'::jsonb,
    audiences     text[] NOT NULL DEFAULT '{}',
    steps         jsonb NOT NULL DEFAULT '[]'::jsonb,
    cooldown_secs bigint NOT NULL DEFAULT 0,
    max_entries   integer,
    starts_at     timestamptz,
    ends_at       timestamptz,
    tags          text[] NOT NULL DEFAULT '{}',
    version       bigint NOT NULL DEFAULT 1,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE journey_runs (
    id          uuid PRIMARY KEY,
    journey_id  uuid NOT NULL REFERENCES journeys (id) ON DELETE CASCADE,
    identity_id uuid NOT NULL REFERENCES identities (id) ON DELETE CASCADE,
    step        integer NOT NULL DEFAULT 0,
    status      text NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'done', 'exited', 'failed')),
    wake_at     timestamptz,
    context     jsonb NOT NULL DEFAULT '{}'::jsonb,
    error       text NOT NULL DEFAULT '',
    entered_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX journey_runs_journey_idx ON journey_runs (journey_id, identity_id, entered_at DESC);
CREATE INDEX journey_runs_identity_idx ON journey_runs (identity_id, entered_at DESC);
CREATE INDEX journey_runs_wake_idx ON journey_runs (wake_at) WHERE status = 'active';

CREATE TABLE journey_run_log (
    id     uuid PRIMARY KEY,
    run_id uuid NOT NULL REFERENCES journey_runs (id) ON DELETE CASCADE,
    step   integer NOT NULL,
    action text NOT NULL,
    detail text NOT NULL DEFAULT '',
    at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX journey_run_log_run_idx ON journey_run_log (run_id, at);
