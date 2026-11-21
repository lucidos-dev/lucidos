# 0627: Reserve minimal content controls before folding optional actions

- **Status**: Accepted
- **Date**: 2026-11-08

## Context

The symmetric reserve introduced for stable navigation truncated content titles before the controls required it.

## Decision

Wide desktop panes reserve the persistent control pairs and fold optional actions progressively. Below 34rem, move optional actions and Refresh into a leading overflow menu. Keep the bell and dashboard toggle trailing, with compact chevrons bracketing the centered title.

The header row spans the pane; edge padding and traffic-light clearance inset controls without shifting the title axis.

On mobile, let only the content title shrink below the shared brand minimum. Keep the span centered on the pane axis at every width.

## Rationale

At the pane floor, trailing overflow, bell and dashboard toggle consume too much of the title’s right half. Moving overflow into the available leading slot restores title room without moving navigation when optional actions appear.

## Consequences

This revises the narrow reserve from ADR 0399 while retaining its wide growth policy. Narrow desktop titles gain two icon boxes and two gaps overall.

Other mobile headers retain the brand minimum. A cramped content title truncates inside a symmetric span, with its chevrons clear of the action pair.

The follow-up centering requirement supersedes the initial leading-space reclaim: an asymmetric shift gave the title more room but moved its text off the pane axis.

## Alternatives considered

Measuring each destination’s visible action width would move the chevrons during navigation. Reserving unfolded actions spends title space before those actions exist.
