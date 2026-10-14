-- Campaign sweeps look up delivered copies by campaign instead of per identity.
CREATE INDEX IF NOT EXISTS inbox_message_occurrence_idx
    ON inbox (message_id, occurrence)
    WHERE message_id IS NOT NULL;
