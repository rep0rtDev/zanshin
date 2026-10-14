-- Journeys v2: live events on a single player's timeline (personal windows).
ALTER TABLE live_event_members
    ADD COLUMN IF NOT EXISTS window_start timestamptz,
    ADD COLUMN IF NOT EXISTS window_end   timestamptz;
