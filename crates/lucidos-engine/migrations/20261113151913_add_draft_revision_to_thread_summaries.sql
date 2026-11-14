-- A thread's draft revision: it goes up by one only when the saved draft
-- changes. An agent names it to send a saved draft (ADR 0651), so activity,
-- a title or a status change must not move it. `summary_version` moves on
-- every row change and stays the client's ordering token (ADR 0329).
--
-- A trigger owns the column, so no writer can forget to bump it and none can
-- set it: an insert starts at 0.
ALTER TABLE thread_summaries
    ADD COLUMN draft_revision BIGINT NOT NULL DEFAULT 0;

-- The drafts read named `summary_version` as the revision before this
-- column existed. Seeding from it means a token read before the upgrade can
-- match only a draft nobody has touched since, never a later edit that
-- counted up to the same number.
UPDATE thread_summaries
   SET draft_revision = summary_version
 WHERE summary_version <> 0;

CREATE FUNCTION thread_summaries_draft_revision_insert() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    NEW.draft_revision := 0;
    RETURN NEW;
END
$$;

CREATE FUNCTION thread_summaries_draft_revision_update() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    NEW.draft_revision := OLD.draft_revision;
    IF (NEW.compose_text, NEW.compose_images, NEW.compose_mode,
        NEW.compose_selection, NEW.compose_epoch, NEW.drafted_by_thread_id)
       IS DISTINCT FROM
       (OLD.compose_text, OLD.compose_images, OLD.compose_mode,
        OLD.compose_selection, OLD.compose_epoch, OLD.drafted_by_thread_id) THEN
        NEW.draft_revision := OLD.draft_revision + 1;
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER thread_summaries_draft_revision_insert
    BEFORE INSERT ON thread_summaries
    FOR EACH ROW EXECUTE FUNCTION thread_summaries_draft_revision_insert();

CREATE TRIGGER thread_summaries_draft_revision_update
    BEFORE UPDATE ON thread_summaries
    FOR EACH ROW EXECUTE FUNCTION thread_summaries_draft_revision_update();
