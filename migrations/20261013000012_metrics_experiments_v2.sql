-- Metrics catalog + experiments v2 (caps, deadline, lock, split key, goal/monitor metrics, copy, interrupt, phases).

CREATE TABLE metrics (
    id          uuid PRIMARY KEY,
    key         text NOT NULL UNIQUE,
    name        text NOT NULL DEFAULT '',
    description text NOT NULL DEFAULT '',
    kind        text NOT NULL DEFAULT 'goal' CHECK (kind IN ('goal', 'monitor')),
    event_name  text NOT NULL,
    aggregation text NOT NULL DEFAULT 'count' CHECK (aggregation IN ('count', 'unique', 'sum', 'avg', 'conversion')),
    field       text NOT NULL DEFAULT '',
    tags        text[] NOT NULL DEFAULT '{}',
    version     bigint NOT NULL DEFAULT 1,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE experiments
    ADD COLUMN split_key        text NOT NULL DEFAULT 'identity',
    ADD COLUMN max_participants bigint,
    ADD COLUMN enrol_until      timestamptz,
    ADD COLUMN locked           boolean NOT NULL DEFAULT false,
    ADD COLUMN metrics          text[] NOT NULL DEFAULT '{}',
    ADD COLUMN copied_from      text,
    ADD COLUMN phase_of         text,
    ADD COLUMN interrupted_at   timestamptz,
    ADD COLUMN interrupt_reason text NOT NULL DEFAULT '',
    ADD COLUMN winner           text;

CREATE INDEX experiment_assignments_assigned_idx ON experiment_assignments (experiment_id, assigned_at);
