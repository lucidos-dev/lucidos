# Governance

This document describes how Lucidos is run today and how that is expected to
change as the community grows. It's deliberately lightweight — Lucidos is a small,
pre-1.0 project — but it's written down so the rules are explicit rather than
implied.

## Today: benevolent-dictator-for-now

Lucidos is currently maintained and governed **solely by Kenneth Tiller**, the
project's creator and **lead maintainer**. There is no maintainer team or shared
decision body yet — Kenneth holds final say over the project's direction,
architecture, and releases. This is the classic "BDFL" model, scoped to the
pre-1.0 phase: it keeps decisions fast and the vision coherent while the project
is still finding its shape. The "for now" is intentional — this is a stage, not a
permanent structure (see *Growing into a maintainer team* below).

> Kenneth is also the person responsible for enforcing the
> [Code of Conduct](CODE_OF_CONDUCT.md) and for receiving private
> [security reports](SECURITY.md).

## Contributors

Lucidos welcomes outside contributions and has already had some — for example,
**Akram** has contributed to the project. The full list is on the
[contributors graph](https://github.com/lucidos-dev/lucidos/graphs/contributors),
derived from git history (every commit is DCO signed-off). Contributors are
credited for their work, but contributing does not by itself confer maintainer
status or a role in governance; decision-making currently rests entirely with the
lead maintainer. See *Growing into a maintainer team* for how that can change over
time.

## How decisions are made

- **Everyday changes** (bug fixes, docs, self-contained features) go through
  normal pull-request review: the lead maintainer reviews, approves, and merges.
  Releases are cut locally via `scripts/release.sh` — there is no public CI gate,
  so review relies on the contributor running the relevant tests locally and
  saying so in the PR.
- **Significant or hard-to-reverse decisions** (architecture, public surfaces,
  removing a capability, anything the codebase pointedly *doesn't* do) are
  discussed in the open — on the issue, the PR, or in
  [GitHub Discussions](https://github.com/lucidos-dev/lucidos/discussions) — and
  recorded as an **Architecture Decision Record** under
  [`docs/adr/`](docs/adr/README.md). Check the ADRs before re-opening a settled
  question; the *why* is usually already written down.
- **The final call rests with the lead maintainer.** Input from contributors is
  welcome and actively encouraged, but until there is a maintainer team, Kenneth
  makes the deciding call. The goal is to keep moving, not to litigate.

## Growing into a maintainer team

The single-leader model is a starting point, not the destination. As more people
contribute meaningfully and consistently, Lucidos will transition toward a
**team of maintainers** who share decision-making authority, with the lead
maintainer's role shifting from "decides everything" toward "breaks ties and
guards the vision."

### Becoming a maintainer

There's no application form. Maintainership is **earned through sustained,
high-quality contribution** and offered by invitation from the lead maintainer.
The things that build the case:

- **A track record of merged contributions** — code, documentation, knowhow,
  reviews — that show good judgment and care for the project.
- **Understanding of the project's shape** — its vocabulary (see the
  [glossaries](docs/glossary.md)), its architecture, and the decisions captured
  in [`docs/adr/`](docs/adr/README.md). Maintainers are expected to keep
  documentation and `system-knowhow/` in sync with the code they change.
- **Good citizenship** — helpful, respectful participation in issues, reviews,
  and discussions, consistent with the [Code of Conduct](CODE_OF_CONDUCT.md).
- **Reliability** — following through on what you take on, and reviewing others'
  work constructively.

When someone has been contributing at that level, the lead maintainer can invite
them to become a maintainer and grant the corresponding access.

### As the team grows

Once there is a maintainer team large enough that single-leader decisions no
longer fit, this document will be updated to describe the team-based model that
replaces it — for example, a defined decision quorum, an RFC process for major
changes, and a clear scope for the lead maintainer's tie-breaking role. Changes
to *this* governance document follow the same "significant decision" process
described above, and are themselves made in the open.

## Amending this document

Proposals to change how Lucidos is governed are welcome. Open an issue or a
discussion, and — as with any significant decision — the change is discussed
openly. Until the team-based model is in place, the lead maintainer makes the
final decision.
