-- A thread summary's version: it goes up by one on every change to the row,
-- whoever makes it. The client keeps the highest version it has seen and
-- never applies an older summary, so a stale read cannot overwrite a fresh
-- one. See ADR 0329.
--
-- A trigger owns the column, so no writer can forget to bump it and none can
-- set it: an insert starts at 0, and an update that changes nothing keeps it.
ALTER TABLE thread_summaries
    ADD COLUMN summary_version BIGINT NOT NULL DEFAULT 0;

CREATE FUNCTION thread_summaries_version_insert() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    NEW.summary_version := 0;
    RETURN NEW;
END
$$;

CREATE FUNCTION thread_summaries_version_update() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    NEW.summary_version := OLD.summary_version;
    IF NEW IS DISTINCT FROM OLD THEN
        NEW.summary_version := OLD.summary_version + 1;
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER thread_summaries_version_insert
    BEFORE INSERT ON thread_summaries
    FOR EACH ROW EXECUTE FUNCTION thread_summaries_version_insert();

CREATE TRIGGER thread_summaries_version_update
    BEFORE UPDATE ON thread_summaries
    FOR EACH ROW EXECUTE FUNCTION thread_summaries_version_update();
