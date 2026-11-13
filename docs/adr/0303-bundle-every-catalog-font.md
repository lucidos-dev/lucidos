# 0303: Lucidos bundles every catalog font; none loads from a third party, so a look may name any of them

- **Status**: Accepted
- **Date**: 2026-09-27
- **Builds on**: [ADR 0077](0077-default-ui-font-is-vendored.md), [ADR 0298](0298-look-fonts-make-no-third-party-request.md)
- **Reverses**: ADR 0298's rejected alternative "Vendor every catalog font"

## Context

ADR 0077 vendored the default font and left three opt-in fonts on the Google
Fonts CDN: Inter, JetBrains Mono and IBM Plex Mono. ADR 0298 then barred a look
from naming a CDN font. So a look could not suggest three fonts that users
could pick.

The catalog also grew by eight fonts: Roboto, Open Sans, Manrope, Lora,
Literata, Source Code Pro, Commit Mono and Cascadia Code. Two of those are not
on Google Fonts at all.

Plan: `docs/plans/2026-09-27-bundle-every-catalog-font.md`.

## Decision

Every catalog font is vendored or a device font. None loads from a third party.
The `cdn` font source is removed.

## Rationale

- **Offline.** A bundled font renders on a workspace with no internet. A CDN
  font rendered in its fallback stack.
- **Privacy.** A CDN font told Google about every boot of every device that
  picked it. The user's own pick is still a request the user did not know they
  were making.
- **Looks can name any font.** ADR 0298's rule stays true by construction:
  there is no longer a font a look must refuse.

ADR 0077 and ADR 0298 rejected this for its weight. The maintainer ruled that
about 100 KB per font is nothing next to the engine binary, which removes the
only argument against it.

## Consequences

- About 956 KB of woff2 is added, carried twice: in the engine binary and in
  the frontend build. So about 1.9 MB per install.
- Nothing downloads a face until text uses it, as before.
- `GET /api/v1/fonts` keeps `look_nameable`, now `true` for every font. Apps
  that filter on it keep working.
- The host drops its Google Fonts preconnect.
- A font without a variable version ships static weights: IBM Plex Mono (400 to
  700) and Commit Mono (400, 700).
- A font whose licence reserves its name is never subset by us. Cascadia Code
  ships as its release file, full character set. Google's own subsets of Plex,
  Lora and Source fonts ship unmodified.
- Adding a font means vendoring it. There is no cheaper path to reach for.

## Alternatives considered

- **Keep the `cdn` source for a future font.** It would be a variant nothing
  uses, with a code path in every surface that only tests exercise. Bringing it
  back costs one enum variant, and the decision to do so would need an ADR
  anyway.
- **Keep the three fonts on the CDN and vendor only the new ones.** Looks could
  still not name Inter or JetBrains Mono, and the privacy leak would stay for the
  users who picked them.
