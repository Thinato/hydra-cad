-- Blocks become versioned, immutable catalogue rows like valve types, and an owner (valve type
-- or block row) has at most one display and one source model.

ALTER TABLE blocks
    ADD COLUMN key text CHECK (key <> '' AND key = btrim(key) AND position('@' in key) = 0),
    ADD COLUMN version integer NOT NULL DEFAULT 1 CHECK (version > 0),
    ADD CHECK (jsonb_typeof(mount_faces) = 'array'),
    ADD UNIQUE (key, version);
UPDATE blocks SET key = 'block-' || id;
ALTER TABLE blocks ALTER COLUMN key SET NOT NULL;

CREATE TRIGGER blocks_immutable BEFORE UPDATE OR DELETE ON blocks
    FOR EACH ROW EXECUTE FUNCTION refuse_catalogue_edit();

CREATE UNIQUE INDEX part_assets_one_per_role ON part_assets (owner_id, role);
