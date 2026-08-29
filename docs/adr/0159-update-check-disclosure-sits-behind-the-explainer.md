# 0159: The update check's disclosure sits behind the explainer, not at rest on the page

- **Status**: Accepted
- **Date**: 2026-08-29

Amends [ADR 0139](0139-update-check-runs-on-notice-not-a-click.md) on one point:
where the Settings row says what the check sends.

## Context

ADR 0139 removed the first-run notice. It made `PRIVACY.md` the notice and the
Settings switch the control. It added one clause about placement. The Settings
row is the only place the product itself speaks. So the row states what is sent
and how often **in the open**, rather than only behind its explainer.

That shipped as a permanent grey paragraph under the switch. It lists the
platform, the architecture, the version and the IP address any web request
carries. Settings > System is otherwise a page of plain rows, and this is the
only one trailing a block of prose.

## Decision

**The paragraph leaves the page and the explainer carries it.** The row is a
label, an info icon and a switch, like every other row. The explainer's first
paragraph says how often the check runs and what it sends.

**The wording is levelled down to what it is: a regular web request.** It names
the platform, the architecture and the version. It no longer spells out the IP
address on first contact, because that frames something every request on the
machine already does as a warning.

Everything else in ADR 0139 stands, the payload included.

## Rationale

**ADR 0139's own argument reaches the Settings row.** It rejected the first-run
notice partly because a new user's first screen was a warning about a network
request. Leading with a paragraph about IP addresses frames a version check as
something to defend against. A paragraph under the switch makes the same frame,
to the same reader, on a page they open for the version numbers.

**The explainer is the app's answer to standing grey prose.** It exists to hold
copy about one control (`docs/glossary.md` § explainer), and every other
explained row on the page already uses it. Keeping one row's copy at rest made
that row look like it carried a warning the others did not.

**`PRIVACY.md` is still the full statement, and it is unchanged.** It sets out
the three values in a table and the IP address our CDN sees. It says what we do
with the counts, and how to turn the check off. Nothing about disclosure is lost
by summarising it one tap away.

**A summary must stay true.** "Nothing else is sent" is about the payload, and
the payload is still platform, architecture and version. The IP address is a
property of making a request at all, which is what "a regular web request" says.

## Consequences

- **The disclosure is one tap away rather than in the reader's eye.** That is the
  point, and it is also the cost. A reader who never opens the explainer learns
  what the check sends from `PRIVACY.md` instead.
- **ADR 0139's placement clause no longer describes the code.** This record is
  why, and 0139 points here.
- **The wording obligation is unchanged.** `PRIVACY.md` and this row are still
  the only places the product discloses this, and both must stay accurate.
- **The row loses its `.settings-row-note`, the last one on the page.** Settings
  > System is uniform now: every explained row hangs its copy on its own label.

## Alternatives considered

**Keep the paragraph and only soften its wording.** It fixes the tone and leaves
the shape. Rejected because the shape is half the problem: one row of grey prose
among plain rows reads as a caveat whatever it says.

**Move it to the section heading's explainer.** The copy is about one control, so
by the explainer's own scope rule it hangs on that control's label. A heading
explainer would answer a question nobody asked at the heading.

**Drop the sentence about what is sent and link `PRIVACY.md`.** The app ships no
reader for that file, so the link would leave the product saying nothing at all.
ADR 0139 is explicit that the row is where the product speaks.

**Say nothing about the IP address anywhere in the app.** This is what shipped,
and it is defensible only because `PRIVACY.md` says it plainly. That page calls
the short version a half-truth. Were it ever to drop the point, the explainer
would have to take it back.
