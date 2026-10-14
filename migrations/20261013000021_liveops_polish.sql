-- LiveOps polish: integration health, experiment priority on shared flags, inbox opt-out.

ALTER TABLE integrations
    ADD COLUMN IF NOT EXISTS last_status   text,
    ADD COLUMN IF NOT EXISTS last_error    text,
    ADD COLUMN IF NOT EXISTS last_at       timestamptz,
    ADD COLUMN IF NOT EXISTS sent_count    bigint NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS failed_count  bigint NOT NULL DEFAULT 0;

ALTER TABLE experiments
    ADD COLUMN IF NOT EXISTS priority integer NOT NULL DEFAULT 0;

ALTER TABLE messages
    ADD COLUMN IF NOT EXISTS inbox boolean NOT NULL DEFAULT true;
