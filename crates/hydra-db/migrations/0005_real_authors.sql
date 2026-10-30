-- Everyone signs in with their own role, so "who did it" columns are set by the database from
-- that role, whatever a client sends; and stored files never change under their hash.

CREATE FUNCTION stamp_circuit_owner() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        NEW.owner := current_user;
    ELSE
        NEW.owner := OLD.owner;
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER circuits_owner BEFORE INSERT OR UPDATE ON circuits
    FOR EACH ROW EXECUTE FUNCTION stamp_circuit_owner();

CREATE FUNCTION stamp_revision_author() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    NEW.created_by := current_user;
    RETURN NEW;
END
$$;

CREATE TRIGGER circuit_revisions_author BEFORE INSERT ON circuit_revisions
    FOR EACH ROW EXECUTE FUNCTION stamp_revision_author();

-- Fires before catalogue_proposals_reviewed_once (triggers run in name order), which then
-- sees the reviewer set.
CREATE FUNCTION stamp_proposal_reviewer() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.status = 'pending' AND NEW.status <> 'pending' THEN
        NEW.reviewed_by := current_user;
        NEW.reviewed_at := now();
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER catalogue_proposals_by_reviewer BEFORE UPDATE ON catalogue_proposals
    FOR EACH ROW EXECUTE FUNCTION stamp_proposal_reviewer();

CREATE FUNCTION refuse_asset_edit() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'stored files never change; store the new file as a new asset';
END
$$;

CREATE TRIGGER assets_immutable BEFORE UPDATE OR DELETE ON assets
    FOR EACH ROW EXECUTE FUNCTION refuse_asset_edit();
CREATE TRIGGER assets_kept BEFORE TRUNCATE ON assets
    FOR EACH STATEMENT EXECUTE FUNCTION refuse_asset_edit();
