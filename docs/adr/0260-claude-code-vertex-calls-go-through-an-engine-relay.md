# 0260: Claude Code's Vertex calls go through an engine relay that asks always-thinking models for progress notes

- **Status**: Accepted
- **Date**: 2026-09-23

## Context

Opus 5.5 and Fable 5.x write their notes between tool calls into `thinking`
blocks. The blocks come back empty unless the request sets
`thinking.display: "updates"` and sends the matching beta header. Claude Code
2.1.280 sets both only when its provider is `firstParty`. On Vertex it never
does, and no flag or variable changes that.

So a coding agent's prose before a tool call reached nobody. The user saw a
blank step, most often right before a question card. The first answer was a
prompt stopgap: put the prose inside the card's question. Cards render inline
markdown only, so users lost lists, headings and code blocks.

## Decision

The engine runs a relay on `127.0.0.1` and points each Claude Code session's
`ANTHROPIC_VERTEX_BASE_URL` at it. For an always-thinking model, the relay sets
`display: "updates"` and adds the beta. It forwards everything else unchanged.
The Claude Code parser then renders each note as the agent's message.

## Rationale

- **It fixes the cause, not the symptom.** The notes exist; the request just
  never asks for them. Asking restores them everywhere, including the 479 blank
  steps before `Bash` calls that the card stopgap never reached.
- **Claude Code already supports the hook.** It honours
  `ANTHROPIC_VERTEX_BASE_URL` and appends the Vertex path, region included. A
  throwaway relay proved the whole chain before any engine code was written.
- **The change to the request is one field.** The body parses as raw values
  and only `thinking` is replaced. The conversation, system prompt and tools
  keep their exact bytes, so prompt caching behaves as before.
- **The Lucidos Agent already does the same.** Both agents now share one rule
  for what counts as a note (`anthropic_wire::progress_note`).

## Consequences

- **A second listener.** It is plain HTTP on loopback with an ephemeral port,
  so it needs no certificate and never sees the LAN. Routes still sit under
  `/api/v1/`.
- **The user's Google token crosses the relay.** The relay stores no
  credential, forwards `Authorization` unchanged and logs no header.
- **Every session gets a signed relay token** in its base URL. It is an HMAC
  under the per-start origin secret, in its own domain, so it never passes as
  an origin token. It names the thread and any Vertex base URL the user had set.
- **The relay only reaches Vertex.** It accepts only the Vertex model path. The
  host comes from a validated region or from the signed override, never from
  the request.
- **An upstream failure stays loud.** The relay passes the status and body
  through, or answers 502 in the API's error shape. Claude Code keeps its own
  retries.
- **It is a temporary measure.** `docs/temporary-measures.md` § "Claude Code's
  Vertex calls go through the Vertex relay" holds the repro that says when
  Claude Code fixed it upstream.

## Alternatives considered

- **Block markdown in question cards.** Cheap, and the user's first complaint.
  It keeps the stopgap, which leaves every note before a non-card tool call
  blank. Rejected by the user in favour of the relay.
- **Wait for Claude Code to fix it.** No engine code, but no date either. The
  gate is a hard `firstParty` check in 2.1.280, the latest release.
- **Run coding agents on Opus 5.** Opus 5 writes its notes as `text`. Users
  lose Opus 5.5 for coding work, which is a worse trade.
- **Serve the relay on the main API socket.** Claude Code's Node runtime does
  not trust the engine's self-signed dev certificate. That socket also carries
  the LAN bind and `local_auth`, neither of which fits a loopback-only hop.
- **Keep a registry of session tokens.** It would need cleanup on every session
  end. A signed, stateless token needs none.
