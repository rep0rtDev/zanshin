-- Role-based admin access: admin (everything), editor (mutate catalog, no key management),
-- viewer (read only). Existing read-only keys become viewers.
ALTER TABLE api_keys
    ADD COLUMN role text NOT NULL DEFAULT 'admin'
      CHECK (role IN ('admin', 'editor', 'viewer'));
UPDATE api_keys SET role = 'viewer' WHERE read_only;
