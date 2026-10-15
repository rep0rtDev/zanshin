-- Console UI language preference per console user (NULL = browser default).
ALTER TABLE console_users ADD COLUMN locale text;
