-- Catalogue proposals get a review: an engineer approves (possibly after editing) or rejects
-- each one once. Reviewed proposals are history and never change or disappear.

ALTER TABLE catalogue_proposals
    ADD COLUMN review_note    text,
    ADD COLUMN reviewed_at    timestamptz,
    -- What was approved, which may differ from `proposal` by the reviewer's edits.
    ADD COLUMN approved       jsonb;

-- Rows from before reviews existed, if any, are made to fit the rule below.
UPDATE catalogue_proposals SET
    reviewed_by = CASE WHEN status = 'pending' THEN NULL
                       ELSE COALESCE(reviewed_by, 'unknown') END;

ALTER TABLE catalogue_proposals
    ADD CONSTRAINT catalogue_proposals_reviewed_by_iff_reviewed
        CHECK ((status = 'pending') = (reviewed_by IS NULL));

-- A pending proposal may be reviewed once: only its review columns change, to approved or
-- rejected, with a reviewer. Anything else, and every delete, is refused.
CREATE FUNCTION refuse_proposal_edit() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND OLD.status = 'pending'
       AND NEW.status IN ('approved', 'rejected') AND NEW.reviewed_by IS NOT NULL
       AND to_jsonb(NEW) - '{status,reviewed_by,reviewed_at,review_note,approved}'::text[]
           = to_jsonb(OLD) - '{status,reviewed_by,reviewed_at,review_note,approved}'::text[] THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION 'proposals are never edited or deleted; a pending one is approved or rejected once';
END
$$;

CREATE TRIGGER catalogue_proposals_reviewed_once BEFORE UPDATE OR DELETE ON catalogue_proposals
    FOR EACH ROW EXECUTE FUNCTION refuse_proposal_edit();
CREATE TRIGGER catalogue_proposals_kept BEFORE TRUNCATE ON catalogue_proposals
    FOR EACH STATEMENT EXECUTE FUNCTION refuse_proposal_edit();

-- A new proposal starts pending and unreviewed, submitted by whoever inserts it, so history
-- can't be written as already reviewed.
CREATE FUNCTION start_proposal() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.status <> 'pending' OR NEW.reviewed_by IS NOT NULL OR NEW.reviewed_at IS NOT NULL
       OR NEW.review_note IS NOT NULL OR NEW.approved IS NOT NULL THEN
        RAISE EXCEPTION 'a new proposal starts pending; it is reviewed afterwards';
    END IF;
    NEW.created_by := current_user;
    RETURN NEW;
END
$$;

CREATE TRIGGER catalogue_proposals_start_pending BEFORE INSERT ON catalogue_proposals
    FOR EACH ROW EXECUTE FUNCTION start_proposal();
