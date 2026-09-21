-- Seed Claude Fable 5.1 into the chat model registry.
--
-- Served over the direct `anthropic` provider under the bare first-party id
-- `claude-fable-5-1`, exactly like the Fable 5 rows. The `[1m]` suffix is the
-- Lucidos convention selecting the 1M-context beta, stripped before the
-- request by anthropic_wire::parse_context_suffix.
--
-- WHY THE SORT ORDERS ARE NEGATIVE
--
-- Fable 5 holds 0 and 1, and Fable 5.1 supersedes it, so the new rows belong
-- above. No free non-negative integer sits there. Renumbering Fable 5 is the
-- alternative and it is ruled out: sort_order is user-editable through the
-- models API, so an UPDATE here would clobber somebody's own ordering. The
-- column is a plain INTEGER read with ORDER BY sort_order ASC, and nothing
-- constrains its sign.
--
-- Builtin = disable-only (never deletable), enabled by default. ON CONFLICT DO
-- NOTHING so re-running against a populated table is a no-op.
INSERT INTO models (id, label, provider, sort_order, source) VALUES
  ('claude-fable-5-1',     'Fable 5.1',      'anthropic', -2, 'builtin'),
  ('claude-fable-5-1[1m]', 'Fable 5.1 (1M)', 'anthropic', -1, 'builtin')
ON CONFLICT (id) DO NOTHING;

-- Declare the window on the `[1m]` row only, matching the contract set by
-- 20260725211150: a declared window describes the window of the request
-- Lucidos actually makes, not the model's theoretical maximum. The `[1m]` id
-- does request 1M mode, so 1000000 is honest.
--
-- The bare row stays NULL on purpose so it keeps tracking the prefix map's
-- 200k. A bare id sends no `context-1m-2025-08-07` beta, and declaring 1M
-- there would let the context packer build a prompt larger than the API mode
-- the request selected, which the provider then rejects outright.
--
-- `source = 'builtin'` keeps the ID-conflict case a true no-op. The INSERT
-- above already declines to overwrite a row the user created under this id, so
-- without this condition the UPDATE would still reach in and set a window (and
-- bump updated_at) on somebody's own row.
UPDATE models SET context_window = 1000000, updated_at = NOW()
WHERE context_window IS NULL
  AND source = 'builtin'
  AND id = 'claude-fable-5-1[1m]';
