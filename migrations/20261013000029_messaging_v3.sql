-- Messaging v3 + Journeys v3: frequency caps, quiet hours, copy variants, rich push, deferrals,
-- journey goals / exit criteria / split branches.

ALTER TABLE messages
    ADD COLUMN IF NOT EXISTS frequency_cap jsonb,
    ADD COLUMN IF NOT EXISTS quiet_hours   jsonb,
    ADD COLUMN IF NOT EXISTS variants      jsonb NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN IF NOT EXISTS push_options  jsonb;

ALTER TABLE message_deliveries
    ADD COLUMN IF NOT EXISTS variant text NOT NULL DEFAULT '';

ALTER TABLE message_deliveries DROP CONSTRAINT IF EXISTS message_deliveries_status_check;
ALTER TABLE message_deliveries
    ADD CONSTRAINT message_deliveries_status_check
    CHECK (status IN ('sent', 'failed', 'skipped', 'capped', 'deferred'));

CREATE INDEX IF NOT EXISTS message_deliveries_identity_sent_idx
    ON message_deliveries (identity_id, at) WHERE channel = 'inbox' AND status = 'sent';

CREATE TABLE IF NOT EXISTS message_deferrals (
    id          uuid PRIMARY KEY,
    message_id  uuid NOT NULL,
    identity_id uuid NOT NULL,
    occurrence  timestamptz,
    due_at      timestamptz NOT NULL,
    reason      text NOT NULL DEFAULT '',
    created_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (message_id, identity_id, occurrence)
);
CREATE INDEX IF NOT EXISTS message_deferrals_due_idx ON message_deferrals (due_at);

ALTER TABLE journeys
    ADD COLUMN IF NOT EXISTS goal      jsonb,
    ADD COLUMN IF NOT EXISTS exit_when jsonb;

ALTER TABLE journey_runs
    ADD COLUMN IF NOT EXISTS converted_at timestamptz,
    ADD COLUMN IF NOT EXISTS branch       text NOT NULL DEFAULT '';

-- Native email providers.
ALTER TABLE integrations DROP CONSTRAINT IF EXISTS integrations_kind_check;
ALTER TABLE integrations
    ADD CONSTRAINT integrations_kind_check
    CHECK (kind IN ('webhook', 'onesignal', 'fcm', 'apns', 'facebook', 'smtp', 'ses', 'sendgrid', 'resend'));
