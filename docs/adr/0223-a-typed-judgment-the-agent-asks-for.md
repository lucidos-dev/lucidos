# 0223: The agent can ask for a typed judgment itself, through a gated judge tool

- **Status**: Accepted
- **Date**: 2026-09-19

## Context

[ADR 0220](0220-jev-is-a-judgment-provider-not-an-llm-provider.md) gave Lucidos
a `JudgmentProvider` trait beside `LlmProvider`, and switched two fixed
classification sites onto it behind one preference each. It also gave apps a
route, the builtin proxy `lucidos.proxy('typesafe')`.

The main agent loop had none. `proxy_request` resolves only names in
`data/config/apis.json`, and the builtin fallback lives in the HTTP route, so
the tool returns a 404 for `typesafe`. The one remaining route was
`http_request` with the key typed into the tool arguments, which writes the
secret into the transcript.

## Decision

One tool, `judge`, taking a state and a set of typed questions and returning
the answers with their probability distributions. It is a tool-capability gate
in the ADR 0088 registry, open when the TypeSafe master switch is on and a key
resolves. There is no third preference.

## Rationale

**ADR 0220's headline rationale does not transfer, and this is what replaces
it.** There, the distribution moves a tie-break out of prompt text and into a
Rust threshold. A tool hands the distribution back to the model, which reasons
about it in prose. Three other things earn the tool its place:

- **Batching.** Independent questions over one state are answered in parallel
  upstream. One call asking fifty questions is one round trip.
- **Calibration.** A frontier model's self-reported confidence is not a
  probability. A `Noul` answer is.
- **Cost.** It is a cheap pre-filter in front of expensive work, where the agent
  would otherwise spend a turn per item.

**The description's main job is to stop a fan-out.** The engine runs a round's
tool calls one after another (`agentic_loop/run.rs`), so fifty calls cost fifty
round trips and fifty slots against `max_tool_calls`. So the schema carries the
batching rule in capitals, with the shape for scoring many items from one
state. A test pins that the rule survives an edit.

**No third preference, because the tool replaces nothing.** ADR 0220's "a
credential is not consent" governs a decision the engine already makes, where
installing a key would silently move a backend. Nothing moves here: the two
classification sites keep running their own path while the tool is offered, and
a test asserts it. A capability appearing once its backend is configured is
`Gate::ImageProvider` and `Gate::EmailAccount`, the shape the registry already
has. A third switch would have sat inside the TypeSafe block, directly under
the master switch, governing no feature page of its own.

**A failure is reported, never answered around.** Both existing sites fall
through to their own prompt-and-parse path when Jev errors. This caller has no
other path. Inventing an answer would be a lie about a number, so the error
reaches the model verbatim and it decides what to do.

## Consequences

- One type serves the argument schema and the wire body. `Question` gained
  `Deserialize` and `Answer` gained `Serialize`, so a second struct mirroring
  either is now a compile error rather than a drift nobody notices.
- **The gate resolves an unknown OPEN, where a classification site resolves it
  shut.** A site has a working chat path, so its doubt is free. Shutting this
  gate withdraws a capability and rewrites the turn's tools cache tier, which
  `read_turn_capabilities` says never to do on an unknown. An absent key still
  closes it, so an unknown never opens the gate alone.
- **The state is composed by the model and sent to a third party.** The two
  existing sites redact before they call, and this one cannot. The description
  names TypeSafe as the recipient. That is guidance, not a guarantee, and it is
  the same posture `web_search` and `http_request` already have.
- The tool's token spend is logged, not recorded as an `AuxCapture` row.
  `AuxCapture::new` wants a thread id and a `ContextPurpose`, and a tool handler
  is given neither. ADR 0220 already calls a new purpose its own change, and no
  other tool reports upstream spend either.
- The row sits last in `FAMILIES`, so opening the gate moves no family already
  on the wire.
- **No system-prompt sentence, unlike `generate_image`.** That one covers
  mechanics its schema cannot, the `thread:N` reference it shares with
  `view_image`. This schema is self-contained, so a prompt sentence would be the
  same guidance in two places, drifting apart on the first edit.
- The context-budget meter bills the gate closed, beside the image one. Both
  open on a credential almost no workspace holds, so billing them would spend
  the ratchet on prose the common workspace is never sent.
- Everything ADR 0220 denied Jev still holds: no `ProviderKind`, no model-picker
  row, no `/health` entry.

## Alternatives considered

**Widen `proxy_request` to fall back to the builtin providers.** The smaller
diff, no new schema bytes, and it reaches Jev. Rejected because the same
fallback hands the agent every provider key the workspace holds, through one
tool call. Reaching one judgment provider is not worth an undifferentiated
route to the OpenAI, Anthropic, Vertex, xAI and local credentials.

**A third per-site preference, `judgment_agent_tool`, default off.** It reads
"a credential is not consent" at its widest, so a key stored for an app adds no
tool until the user asks. Rejected as a control with no home: its only place is
under the master switch it duplicates. The reading it protects is about moving
an existing decision, which this does not do.

**Return only the chosen option.** A smaller tool result, and simpler prose for
the model to read. Rejected because the distribution is the entire reason to
call a judgment provider rather than to think.

**Fall back to the chat model on a Jev failure.** It matches what the two
existing sites do. Rejected because they fall back to a path that already
exists and is tested, where this would mean writing a new one. The agent IS a
chat model, and telling it the call failed lets it decide, which is strictly
more than a fallback could.

**Give it a `ContextPurpose` so the spend lands in the usage capture.** Correct
in the long run. Deferred because ADR 0220 already records that a new purpose
brings a new preference under the one-purpose-per-preference invariant.
