# 0733: Keep the default rung on without rewriting customised ladders

- **Status**: Accepted
- **Date**: 2026-11-18

## Context

A saved ladder can omit a new shipped default. The agent then runs that rung
when no pick is given, yet refuses the same rung when picked explicitly.

## Decision

Read every ladder with its available default on. Leave the saved order
intact and insert a missing default before the first higher estimated rung.
Append it when there is no such rung or the default has no estimate.
Shipped updates never rewrite saved ladders.

## Rationale

A default is an implicit pick. Offering it as an explicit pick keeps both
paths consistent, and placing it at read time preserves the saved order.

## Consequences

The default can be picked by name wherever it can run. Move the star before
turning off the old default or removing its model from the Lucidos Agent's
ladder.

## Alternatives considered

Merging shipped changes into a saved list would override deliberate choices
about its membership and order. Inserting only the default at read time
keeps those choices and lets the default run.
