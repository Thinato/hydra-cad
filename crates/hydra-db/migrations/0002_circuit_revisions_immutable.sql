-- History is never rewritten: a save or a restore inserts a new revision.
ALTER TABLE circuit_revisions ADD CONSTRAINT circuit_revisions_number_positive CHECK (number > 0);

CREATE FUNCTION refuse_revision_edit() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'circuit revisions are never changed; restore creates a new revision';
END
$$;

CREATE TRIGGER circuit_revisions_immutable BEFORE UPDATE OR DELETE ON circuit_revisions
    FOR EACH ROW EXECUTE FUNCTION refuse_revision_edit();
