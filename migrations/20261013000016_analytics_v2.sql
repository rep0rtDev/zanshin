CREATE TABLE dashboards (
    id          uuid PRIMARY KEY,
    key         text NOT NULL UNIQUE,
    name        text NOT NULL DEFAULT '',
    description text NOT NULL DEFAULT '',
    widgets     jsonb NOT NULL DEFAULT '[]'::jsonb,
    filters     jsonb NOT NULL DEFAULT '{}'::jsonb,
    tags        text[] NOT NULL DEFAULT '{}',
    version     bigint NOT NULL DEFAULT 1,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE annotations (
    id         uuid PRIMARY KEY,
    at         timestamptz NOT NULL DEFAULT now(),
    kind       text NOT NULL DEFAULT 'note',
    key        text NOT NULL DEFAULT '',
    title      text NOT NULL,
    body       text NOT NULL DEFAULT '',
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX annotations_at_idx ON annotations (at DESC);
