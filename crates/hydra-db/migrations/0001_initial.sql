-- Hydra catalogue and circuit storage. Catalogue rows are never edited in place:
-- a change inserts a new row with the same `key` and the next `version`. Documents refer to
-- one exact row as `key@version`; uuids stay internal to the database.

CREATE TABLE manufacturers (
    id   uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    key  text NOT NULL UNIQUE CHECK (key <> '' AND key = btrim(key) AND position('@' in key) = 0),
    name text NOT NULL UNIQUE
);

CREATE TABLE symbol_defs (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    kind       text NOT NULL,
    definition jsonb NOT NULL,
    key        text NOT NULL CHECK (key <> '' AND key = btrim(key) AND position('@' in key) = 0),
    version    integer NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (key, version)
);

CREATE TABLE assets (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    sha256     text NOT NULL UNIQUE CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    kind       text NOT NULL CHECK (kind IN ('gltf', 'step', 'pdf')),
    bytes      bytea NOT NULL,
    size       bigint NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE valve_types (
    id                  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    manufacturer_id     uuid NOT NULL REFERENCES manufacturers (id),
    series              text NOT NULL,
    code                text NOT NULL,
    symbol_def_id       uuid REFERENCES symbol_defs (id),
    mounting            text,
    params              jsonb,
    datasheet_asset_id  uuid REFERENCES assets (id),
    key                 text NOT NULL CHECK (key <> '' AND key = btrim(key) AND position('@' in key) = 0),
    version             integer NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at          timestamptz NOT NULL DEFAULT now(),
    UNIQUE (key, version)
);

CREATE TABLE parts (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    valve_type_id uuid NOT NULL REFERENCES valve_types (id),
    part_number   text NOT NULL,
    options       jsonb,
    created_at    timestamptz NOT NULL DEFAULT now(),
    UNIQUE (valve_type_id, part_number)
);

-- Versioned catalogue rows and parts are never updated or deleted. The one exception: linking
-- a datasheet to a valve type, which changes how no circuit draws.
CREATE FUNCTION refuse_catalogue_edit() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND TG_TABLE_NAME = 'valve_types'
       AND to_jsonb(NEW) - 'datasheet_asset_id' = to_jsonb(OLD) - 'datasheet_asset_id' THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'catalogue rows are never edited in place; add a new version';
END
$$;

CREATE TRIGGER symbol_defs_immutable BEFORE UPDATE OR DELETE ON symbol_defs
    FOR EACH ROW EXECUTE FUNCTION refuse_catalogue_edit();
CREATE TRIGGER valve_types_immutable BEFORE UPDATE OR DELETE ON valve_types
    FOR EACH ROW EXECUTE FUNCTION refuse_catalogue_edit();
CREATE TRIGGER parts_immutable BEFORE UPDATE OR DELETE ON parts
    FOR EACH ROW EXECUTE FUNCTION refuse_catalogue_edit();

CREATE TABLE blocks (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name        text NOT NULL,
    mount_faces jsonb NOT NULL DEFAULT '[]',
    created_at  timestamptz NOT NULL DEFAULT now()
);

-- owner_id is a part or a block id, so it carries no foreign key.
CREATE TABLE part_assets (
    owner_id uuid NOT NULL,
    asset_id uuid NOT NULL REFERENCES assets (id),
    role     text NOT NULL CHECK (role IN ('display', 'source')),
    PRIMARY KEY (owner_id, asset_id, role)
);

CREATE TABLE catalogue_proposals (
    id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    datasheet_asset_id uuid REFERENCES assets (id),
    proposal           jsonb NOT NULL,
    status             text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'approved', 'rejected')),
    created_by         text NOT NULL DEFAULT current_user,
    reviewed_by        text,
    created_at         timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE circuits (
    id                  uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name                text NOT NULL,
    project             text,
    owner               text NOT NULL DEFAULT current_user,
    current_revision_id uuid,
    created_at          timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE circuit_revisions (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    circuit_id    uuid NOT NULL REFERENCES circuits (id),
    number        integer NOT NULL,
    document      jsonb NOT NULL,
    created_by    text NOT NULL DEFAULT current_user,
    created_at    timestamptz NOT NULL DEFAULT now(),
    message       text,
    restored_from uuid REFERENCES circuit_revisions (id),
    UNIQUE (circuit_id, number)
);

ALTER TABLE circuits
    ADD FOREIGN KEY (current_revision_id) REFERENCES circuit_revisions (id);
