-- Persist commit subjects for applied changes so the restart-required toast
-- can show "which commits will land on restart" after page reload, not just
-- in the live SSE event.
ALTER TABLE changes ADD COLUMN commits TEXT[] NOT NULL DEFAULT '{}';
