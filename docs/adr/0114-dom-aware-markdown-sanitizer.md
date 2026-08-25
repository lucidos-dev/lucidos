# 0114: Sanitize rendered markdown with DOMPurify, not a hand-rolled walk

- **Status**: Accepted
- **Date**: 2026-08-24

## Context

Raw HTML in markdown source passes through `marked` unescaped. So the string
reaching the frontend mixes the renderer's own markup with whatever the author
typed. `sanitizeHtmlFragments` used to scrub that string by hand: an
`indexOf('<')` walk, a dangerous-tag regex, quote-aware tag extents and a
per-tag attribute scrubber.

The walk carried its own hazard notes about RCDATA elements, raw-text elements
and unterminated comments. Each note marks a place where the scanner's model of
the string and the browser's parse of it disagree. That disagreement is the bug
class, and three separate bypasses were filed against it. It was also a
denylist, so an unlisted dangerous construct passed through untouched.

## Decision

Parse and sanitize with DOMPurify, which uses the browser's own HTML parser.
Keep one policy layer in front of it: a regex pass that escapes a small set of
tags to visible text before the parse.

## Rationale

A second model of HTML is the defect. Patching one more instance of the
divergence leaves the class open, while handing the job to the browser's parser
closes it: there is only one parse, and the sanitizer decides over the tree that
parse produced. DOMPurify is also an allowlist, so an unlisted construct is
removed rather than passed.

Escaping to visible text is a rendering choice, not a security one. DOMPurify's
default is silent removal, and a chat transcript that quietly loses what the
model wrote is the worse default. The pass cannot create markup: escaping only
turns `<`, `>`, `&` and `"` into entities. A tag it misses is still removed by
DOMPurify, so the security decision stays DOMPurify's.

## Consequences

- `dompurify` becomes a runtime dependency in a security path, and `jsdom` a
  development one so the sanitizer can run under Vitest.
- A caller needs a DOM. Without one DOMPurify exports no `sanitize`, so
  `sanitizeHtmlFragments` throws a message naming the `@vitest-environment
  jsdom` docblock.
- Two config additions are load-bearing. `ALLOWED_URI_REGEXP` must carry the
  five app-owned schemes. Each is claimed by an extractor that runs after
  sanitization, and a stripped href leaves it nothing to read. An
  `afterSanitizeAttributes` hook must strip `data:`, which DOMPurify allows on
  `<img>` and friends through `DATA_URI_TAGS` and offers no knob to refuse.
- The escape list carries every element DOMPurify would delete WITH its content,
  except the ones that are real markup inside `<svg>` or `<math>`. A
  context-free regex cannot tell those two contexts apart. So `title`, `desc`
  and the MathML text elements stay out, and lose their content in the HTML
  case.
- Four tests changed, each pinning a serialization artefact of the old scanner
  rather than a behaviour: a bare boolean attribute, an attribute newline, a
  text node's quote, and markup typed inside a `<textarea>`.

## Alternatives considered

**Patch the walk again.** Cheapest per bypass, and the reason the file reached
its old size. Rejected: each fix teaches the scanner one more rule the parser
already knows, and the next divergence is unfiled rather than absent.

**A `uponSanitizeElement` hook instead of the escape pass.** This was the
approved plan, and it was built and rejected. The hook replaces an element with
a text node of its serialized form, which mangles an UNCLOSED raw-text element.
An unclosed `<iframe>` swallows the rest of the document, so serializing its
`outerHTML` emits the trailing markup as visible garbage. Writing
`Sandboxed <iframe> rendering` in chat rendered a stray `</iframe>` after the
paragraph. The pre-pass cannot mangle what it never parses.

**Take DOMPurify's default and remove dangerous tags silently.** Simpler, and
what a stock sanitizer does. Rejected by the user on the approval card: deleting
content from a transcript without a trace is worse than showing the tag.

**A different sanitizer, or none.** No concrete reason against DOMPurify
surfaced. Writing our own DOM-aware sanitizer would re-adopt the maintenance
burden this decision sheds.
