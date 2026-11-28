# 0597: The agent browser navigates only to http and https URLs

- **Status**: Accepted
- **Date**: 2026-10-29

## Context

`browser_open` passed any model-supplied URL to Chrome's `Target.createTarget`.
A `file:` URL followed by `browser_extract` read any local file the engine
user can read. That bypassed the workspace `data/` path limits and the command
guard, with no approval card. `browser_screenshot` takes a `url` too and
navigates through the same `BrowserRuntime::open`.

A URL that CDP opens directly is browser-initiated. So Chrome's own block on a
web page navigating to `file:` or `chrome:` does not apply to it.

## Decision

`BrowserRuntime::open` parses the URL and refuses every scheme except `http`
and `https`, before the browser launches. Chrome receives the parsed,
canonical URL, never the raw string.

## Rationale

- **An allowlist, not a denylist.** Chrome knows schemes a denylist forgets:
  `view-source:`, `filesystem:`, `chrome-extension:`, `devtools:`. Two allowed
  schemes cover every task the tool exists for.
- **Chrome gets what was checked.** Chrome strips tabs, newlines and leading
  control characters before it parses. A check on the raw string would miss a
  scheme split by a tab, and Chrome would then open it. `reqwest::Url` strips
  the same characters, and the canonical form it hands Chrome always starts
  with `http://` or `https://`. No later parse can read it as another scheme.
- **At the runtime, not the tool handler.** Every caller of `open` gets the rule,
  including `screenshot` and any future one.

## Consequences

- The agent cannot open a local HTML artifact in the browser. It reads the file
  with the file tools, and the app frame renders it.
- A page-initiated navigation (an HTTP redirect, a link click, script) stays
  under Chrome's own policy. A web origin cannot navigate a top frame to
  `file:`, `chrome:` or `data:`, so this rule does not repeat that check.
- `browser_evaluate` and `browser_click` act on the current page and add no
  scheme of their own.

## Alternatives considered

- **Denylist `file:`, `data:`, `chrome:` and `javascript:`.** Rejected: it fails
  open on the next scheme nobody listed.
- **Check the final page URL after load as well.** Rejected for now: a failed
  load lands on Chrome's own error page under a non-http scheme. The check would
  turn every DNS failure into a confusing refusal, and Chrome already blocks the
  page-initiated paths.
