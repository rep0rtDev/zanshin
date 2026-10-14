-- Analytics v3: event dimensions for filtering (denormalised from identity default properties at
-- ingest), and advertising spend for CPI / RoAS / LTV.

ALTER TABLE events
    ADD COLUMN IF NOT EXISTS country     text NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS platform    text NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS app_version text NOT NULL DEFAULT '';

CREATE TABLE ad_spend (
    day       date NOT NULL,
    platform  text NOT NULL DEFAULT '',
    country   text NOT NULL DEFAULT '',
    campaign  text NOT NULL DEFAULT '',
    spend     double precision NOT NULL DEFAULT 0,
    installs  integer NOT NULL DEFAULT 0,
    currency  text NOT NULL DEFAULT 'USD',
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (day, platform, country, campaign)
);
