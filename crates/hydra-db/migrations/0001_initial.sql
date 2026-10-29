-- Hydra catalogue and circuit storage. Catalogue rows are never edited in place:
-- a change inserts a new row with the same `lineage_id` and a higher `version`.

CREATE TABLE manufacturers (
    id   uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name text NOT NULL UNIQUE
);

CREATE TABLE symbol_defs (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    kind       text NOT NULL,
    definition jsonb NOT NULL,
    lineage_id uuid NOT NULL DEFAULT gen_random_uuid(),
    version    integer NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (lineage_id, version)
);

CREATE TABLE assets (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    sha256     text NOT NULL UNIQUE,
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
    params              jsonb NOT NULL DEFAULT '{}',
    datasheet_asset_id  uuid REFERENCES assets (id),
    lineage_id          uuid NOT NULL DEFAULT gen_random_uuid(),
    version             integer NOT NULL DEFAULT 1,
    created_at          timestamptz NOT NULL DEFAULT now(),
    UNIQUE (lineage_id, version)
);

CREATE TABLE parts (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    valve_type_id uuid NOT NULL REFERENCES valve_types (id),
    part_number   text NOT NULL,
    options       jsonb NOT NULL DEFAULT '{}',
    created_at    timestamptz NOT NULL DEFAULT now()
);

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
