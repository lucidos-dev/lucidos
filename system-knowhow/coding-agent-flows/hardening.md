---
name: Default flow: hardening
description: The default handoff checkpoint for a review flow. An automatic checkpoint at the flow's first entry, before any review.
---

# Default flow: hardening

A reference of `system-knowhow/coding-agent-flows`, which says how to take it.
Copy the block below to the start of the review flow, markers included.

<!-- default-flow: hardening -->
#### Handoff checkpoint

At this flow's first entry, before any review, run
`lucidos successor offer --at pre-harden --default-floor` and follow what it
prints. Only the first entry runs it: a later pass returns to the review,
never here. A successor session this checkpoint started skips it too, and
starts at the review. `--default-floor` makes the surface's default rung the
floor for this run, so a thread below it hands off onto it.
<!-- /default-flow: hardening -->

## The rest of the flow

The block is the only part Lucidos ships verbatim. The steps after it are the
repository's own, and a review flow usually runs them in this order:

1. Review the whole change against the repository's rules.
2. Fix what the review found, and commit.
3. Run the repository's own verification commands: its linters and test suites.
4. Repeat from step 1 until a pass finds nothing.
