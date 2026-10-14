-- Audiences v2: stored membership, include/exclude lists, managed audiences, recompute schedules.

ALTER TABLE audiences
    ADD COLUMN kind             text NOT NULL DEFAULT 'dynamic' CHECK (kind IN ('dynamic', 'managed')),
    ADD COLUMN recompute        text NOT NULL DEFAULT 'live' CHECK (recompute IN ('live', 'scheduled')),
    ADD COLUMN schedule_cron    text NOT NULL DEFAULT '',
    ADD COLUMN source           text NOT NULL DEFAULT '',
    ADD COLUMN copied_from      text,
    ADD COLUMN last_computed_at timestamptz,
    ADD COLUMN member_count     bigint NOT NULL DEFAULT 0;

-- Current members with the moment they joined. For `live` dynamic audiences this is maintained lazily
-- as identities are evaluated; for `scheduled` and `managed` audiences it is the source of truth.
CREATE TABLE audience_members (
    audience_id uuid NOT NULL REFERENCES audiences (id) ON DELETE CASCADE,
    identity_id uuid NOT NULL REFERENCES identities (id) ON DELETE CASCADE,
    joined_at   timestamptz NOT NULL DEFAULT now(),
    source      text NOT NULL DEFAULT 'rule' CHECK (source IN ('rule', 'include', 'managed')),
    PRIMARY KEY (audience_id, identity_id)
);
CREATE INDEX audience_members_identity_idx ON audience_members (identity_id);

-- Manual include / exclude lists; exclusions may be temporary (`until`).
CREATE TABLE audience_lists (
    audience_id uuid NOT NULL REFERENCES audiences (id) ON DELETE CASCADE,
    identity_id uuid NOT NULL REFERENCES identities (id) ON DELETE CASCADE,
    mode        text NOT NULL CHECK (mode IN ('include', 'exclude')),
    until       timestamptz,
    reason      text NOT NULL DEFAULT '',
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (audience_id, identity_id)
);
CREATE INDEX audience_lists_identity_idx ON audience_lists (identity_id);

-- Default audiences every project starts with.
INSERT INTO audiences (id, key, name, description, condition, tags) VALUES
    (gen_random_uuid(), 'all_players', 'All players', 'Every identity.', '{"all":[]}', '{default}'),
    (gen_random_uuid(), 'new_players', 'New players', 'First seen within the last 7 days.',
        '{"prop":"computed.first_seen_at","op":"within_last","value":604800}', '{default}'),
    (gen_random_uuid(), 'payers', 'Payers', 'Completed at least one purchase.',
        '{"prop":"computed.events.purchaseCompleted.count","op":"gte","value":1}', '{default}'),
    (gen_random_uuid(), 'inactive_7d', 'Inactive 7 days', 'Not seen for more than 7 days.',
        '{"prop":"computed.last_seen_at","op":"older_than","value":604800}', '{default}'),
    (gen_random_uuid(), 'ios', 'iOS', 'Platform is iOS.', '{"prop":"default.platform","op":"eq","value":"ios"}', '{default}'),
    (gen_random_uuid(), 'android', 'Android', 'Platform is Android.', '{"prop":"default.platform","op":"eq","value":"android"}', '{default}')
ON CONFLICT (key) DO NOTHING;
