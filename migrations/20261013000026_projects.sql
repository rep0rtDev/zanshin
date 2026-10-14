-- Multi-project: registry of projects (games / environments) hosted by one deployment. Every
-- non-default project gets its own PostgreSQL schema `zanshin_<slug>`, Redis prefix and
-- ClickHouse database; this table lives in the default project's schema.

CREATE TABLE IF NOT EXISTS projects (
    slug         text PRIMARY KEY,
    name         text NOT NULL DEFAULT '',
    description  text NOT NULL DEFAULT '',
    archived     boolean NOT NULL DEFAULT false,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);

INSERT INTO projects (slug, name, description)
VALUES ('default', 'Default', 'The original single-tenant project (public schema)')
ON CONFLICT (slug) DO NOTHING;
