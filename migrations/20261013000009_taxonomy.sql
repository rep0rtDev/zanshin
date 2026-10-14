-- Taxonomy: category labels, event definitions, value schemas, property configuration,
-- play sessions, identity merge log.

CREATE TABLE labels (
    id          uuid PRIMARY KEY,
    name        text NOT NULL UNIQUE,
    color       text NOT NULL DEFAULT '',
    description text NOT NULL DEFAULT '',
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE event_definitions (
    name                  text PRIMARY KEY,
    description           text NOT NULL DEFAULT '',
    value_type            text NOT NULL DEFAULT 'any'
                            CHECK (value_type IN ('any', 'number', 'string', 'object', 'revenue')),
    metadata_schema       jsonb,
    exclude_from_activity boolean NOT NULL DEFAULT false,
    is_core               boolean NOT NULL DEFAULT false,
    discovered            boolean NOT NULL DEFAULT false,
    tags                  text[] NOT NULL DEFAULT '{}',
    created_at            timestamptz NOT NULL DEFAULT now(),
    updated_at            timestamptz NOT NULL DEFAULT now()
);

INSERT INTO event_definitions (name, description, value_type, is_core) VALUES
    ('sessionStart',      'Play session started',                            'any',     true),
    ('sessionEnd',        'Play session ended; value = duration in seconds', 'number',  true),
    ('purchaseCompleted', 'In-app purchase completed; value = product id',   'revenue', true),
    ('purchaseFailed',    'In-app purchase failed; value = product id',      'string',  true),
    ('adStarted',         'Ad impression started; value = placement',        'string',  true),
    ('adCompleted',       'Ad impression completed; value = placement',      'revenue', true),
    ('adFailed',          'Ad failed to show; value = placement',            'string',  true),
    ('levelStarted',      'Level started; value = level id',                 'any',     true),
    ('levelCompleted',    'Level completed; value = level id',               'any',     true),
    ('levelFailed',       'Level failed; value = level id',                  'any',     true),
    ('tutorialStarted',   'Tutorial step started; value = step',             'any',     true),
    ('tutorialCompleted', 'Tutorial step completed; value = step',           'any',     true),
    ('tutorialSkipped',   'Tutorial skipped; value = step',                  'any',     true),
    ('itemSpent',         'Item spent; value = item id, metadata.amount',    'any',     true),
    ('itemConsumed',      'Item consumed; value = item id',                  'any',     true),
    ('currencyGained',    'Soft/hard currency gained; value = amount',       'number',  true),
    ('currencySpent',     'Soft/hard currency spent; value = amount',        'number',  true);

CREATE TABLE value_schemas (
    id          uuid PRIMARY KEY,
    name        text NOT NULL UNIQUE,
    description text NOT NULL DEFAULT '',
    schema      jsonb NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE flags ADD COLUMN schema_id uuid REFERENCES value_schemas (id) ON DELETE SET NULL;
ALTER TABLE live_events ADD COLUMN schema_id uuid REFERENCES value_schemas (id) ON DELETE SET NULL;

CREATE TABLE property_config (
    key         text PRIMARY KEY,
    namespace   text NOT NULL DEFAULT 'custom' CHECK (namespace IN ('default', 'custom')),
    read_only   boolean NOT NULL DEFAULT false,
    description text NOT NULL DEFAULT '',
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE settings (
    key        text PRIMARY KEY,
    value      jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE play_sessions (
    id           uuid PRIMARY KEY,
    identity_id  uuid NOT NULL REFERENCES identities (id) ON DELETE CASCADE,
    started_at   timestamptz NOT NULL,
    last_seen_at timestamptz NOT NULL,
    ended_at     timestamptz,
    duration_sec integer,
    platform     text NOT NULL DEFAULT '',
    app_version  text NOT NULL DEFAULT ''
);
CREATE INDEX play_sessions_identity_idx ON play_sessions (identity_id, started_at DESC);
CREATE INDEX play_sessions_open_idx ON play_sessions (last_seen_at) WHERE ended_at IS NULL;

CREATE TABLE identity_merges (
    id              uuid PRIMARY KEY,
    from_identity   uuid NOT NULL,
    from_external   text NOT NULL,
    into_identity   uuid NOT NULL,
    into_external   text NOT NULL,
    merged_at       timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX identity_merges_into_idx ON identity_merges (into_identity);
