-- Live events v2: explicit / sticky participation, per-player duration, flag overrides while active.
ALTER TABLE live_events
    ADD COLUMN join_mode          text NOT NULL DEFAULT 'auto' CHECK (join_mode IN ('auto', 'explicit')),
    ADD COLUMN sticky             boolean NOT NULL DEFAULT false,
    ADD COLUMN participation_secs bigint,
    ADD COLUMN flag_overrides     jsonb NOT NULL DEFAULT '{}'::jsonb;

-- Who joined which event and when (explicit joins, sticky membership, participation timers).
CREATE TABLE live_event_members (
    live_event_id uuid NOT NULL REFERENCES live_events (id) ON DELETE CASCADE,
    identity_id   uuid NOT NULL REFERENCES identities (id) ON DELETE CASCADE,
    joined_at     timestamptz NOT NULL DEFAULT now(),
    left_at       timestamptz,
    PRIMARY KEY (live_event_id, identity_id)
);
CREATE INDEX live_event_members_identity_idx ON live_event_members (identity_id);
