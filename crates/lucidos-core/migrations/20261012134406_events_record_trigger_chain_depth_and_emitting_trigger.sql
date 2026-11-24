-- Record, on each events row, the two facts the trigger matcher reads off the
-- live broadcast: the *trigger chain depth* the event dispatches at, and the
-- *emitting trigger id* whose fire emitted it.
--
-- A lagged scheduler replays the rows it missed (ADR 0137 amendment). Without
-- these, a replayed event would dispatch at depth 0 and wake its own trigger.
-- A chain passing through repeated lags would then never reach the depth cap.
--
-- Both nullable, with no backfill. NULL reads as depth 0 and no emitting
-- trigger, which is what every older row meant. Adding a nullable column with
-- no default rewrites nothing, so this is cheap on a large table.

ALTER TABLE events
  ADD COLUMN trigger_chain_depth INTEGER,
  ADD COLUMN emitting_trigger_id TEXT;
