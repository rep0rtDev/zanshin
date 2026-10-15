-- Flags v3: prerequisites between flags, scheduled changes, change requests with approvals,
-- evaluation counters for stale-flag detection.

ALTER TABLE flags
    ADD COLUMN IF NOT EXISTS prerequisites jsonb NOT NULL DEFAULT '[]'::jsonb;

-- A flag patch that is applied at a point in time by the housekeeping job.
CREATE TABLE IF NOT EXISTS flag_schedules (
    id          uuid PRIMARY KEY,
    flag_key    text NOT NULL,
    at          timestamptz NOT NULL,
    patch       jsonb NOT NULL,
    status      text NOT NULL DEFAULT 'pending',
    created_by  text NOT NULL DEFAULT '',
    note        text NOT NULL DEFAULT '',
    error       text NOT NULL DEFAULT '',
    created_at  timestamptz NOT NULL DEFAULT now(),
    applied_at  timestamptz
);
CREATE INDEX IF NOT EXISTS flag_schedules_due_idx ON flag_schedules (at) WHERE status = 'pending';

-- Proposed configuration changes that need a second pair of eyes before they are applied.
CREATE TABLE IF NOT EXISTS change_requests (
    id                  uuid PRIMARY KEY,
    kind                text NOT NULL,
    key                 text NOT NULL,
    body                jsonb NOT NULL,
    title               text NOT NULL DEFAULT '',
    description         text NOT NULL DEFAULT '',
    status              text NOT NULL DEFAULT 'open',
    author              text NOT NULL DEFAULT '',
    approvals           jsonb NOT NULL DEFAULT '[]'::jsonb,
    required_approvals  integer NOT NULL DEFAULT 1,
    error               text NOT NULL DEFAULT '',
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    applied_at          timestamptz
);
CREATE INDEX IF NOT EXISTS change_requests_status_idx ON change_requests (status, created_at DESC);

-- Daily evaluation counts per flag and served variant (flushed by housekeeping).
CREATE TABLE IF NOT EXISTS flag_eval_stats (
    flag_key  text NOT NULL,
    variant   text NOT NULL DEFAULT '',
    day       date NOT NULL,
    count     bigint NOT NULL DEFAULT 0,
    PRIMARY KEY (flag_key, variant, day)
);
