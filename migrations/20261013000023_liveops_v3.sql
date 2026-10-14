-- Live events v3: variants inside an event and attached monitor metrics.
ALTER TABLE live_events
    ADD COLUMN IF NOT EXISTS variants jsonb NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN IF NOT EXISTS metrics  text[] NOT NULL DEFAULT '{}';
