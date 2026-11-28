# 0709: Idle video players use host-captured posters

- **Status**: Accepted
- **Date**: 2026-11-15

## Context

An on-screen video player retained a paused decoder to show its first frame.
Several widgets can share an iOS content process and its memory limit.
Opaque app frames cannot read a workspace video into a canvas or use Cache Storage.

## Decision

Before its first playback intent, a built-in video player holds no media pipeline.
The host captures a scaled JPEG poster through `lucidos.media.poster`.
Play, a scrub or full screen first attaches the player's video source.
After intent, the existing off-screen release and reload behaviour stays.

## Rationale

A still image preserves the first frame without retaining a decoder per widget.
One host queue decodes at most one poster at a time and always releases it.
A credential-less HEAD checks the frame's resource URL before cache lookup or capture.
A refused request never decodes with the host's credentials.

## Consequences

Posters fit the box times device pixel ratio, capped at 1280 pixels.
Bounded memory and Cache Storage caches reuse a path and validator pair.
A URL without ETag or Last-Modified is captured again.
Off-screen media release remains in place.

External HTTPS clips without CORS show the stage and play button without a poster.
They still play on intent.
Workspace apps can adopt the SDK method; their existing videos are unchanged.
Frame resource reports expose counts and pixels, never resource paths.

## Alternatives considered

- Keeping idle videos loaded preserves their decoder and layer costs.
- Frame-side capture taints the canvas at an opaque origin.
- Cross-frame locks and caches are unavailable at an opaque origin.
- Changing the frame cap cannot help two distinct players already on screen.
