# Governance

This document describes how Lucidos is run today and how that will change as
the community grows.

## Today: benevolent-dictator-for-now

Lucidos is currently maintained and governed **solely by Kenneth Tiller**, the
project's creator and **lead maintainer**. Kenneth holds final say over the
project's direction, architecture, and releases. This "BDFL" model covers the
pre-1.0 phase. *Growing into a maintainer team* below describes what follows it.

> Kenneth is also the person responsible for enforcing the
> [Code of Conduct](CODE_OF_CONDUCT.md) and for receiving private
> [security reports](SECURITY.md).

## Contributors

Lucidos accepts outside contributions. For example, **Akram** has contributed to
the project. The full list is on the
[contributors graph](https://github.com/lucidos-dev/lucidos/graphs/contributors),
derived from git history (every commit is DCO signed-off). Contributors are
credited for their work. Maintainer status and a role in governance come by
invitation, as *Growing into a maintainer team* describes. Until then, the lead
maintainer makes decisions.

## How decisions are made

- **Everyday changes** (bug fixes, docs, self-contained features) go through
  normal pull-request review. The lead maintainer reviews the PR and imports it
  into a release (see [CONTRIBUTING.md](CONTRIBUTING.md#how-this-repository-works)).
  Releases are cut locally via `scripts/release.sh`. Review relies on the
  contributor running the relevant tests locally and saying so in the PR.
- **Significant or hard-to-reverse decisions** are discussed in the open: on the
  issue, the PR, or in
  [GitHub Discussions](https://github.com/lucidos-dev/lucidos/discussions).
  These cover architecture, public surfaces, removing a capability, and anything
  the codebase pointedly *doesn't* do. Each one is recorded as an
  **Architecture Decision Record** under [`docs/adr/`](docs/adr/README.md).
  Check the ADRs before re-opening a settled question.
- **The final call rests with the lead maintainer.** Contributors give input,
  and until there is a maintainer team, Kenneth makes the deciding call.

## Growing into a maintainer team

As more people contribute consistently, Lucidos will move to a **team of
maintainers** who share decision-making authority. The lead maintainer's role
will then shift to breaking ties and guarding the vision.

### Becoming a maintainer

Maintainership is **earned through sustained, high-quality contribution** and
offered by invitation from the lead maintainer. The lead maintainer looks for:

- **A track record of merged contributions** (code, documentation, knowhow,
  reviews) that shows good judgment and care for the project.
- **Understanding of the project's shape**: its vocabulary (see the
  [glossaries](docs/glossary.md)), its architecture, and the decisions captured
  in [`docs/adr/`](docs/adr/README.md). Maintainers keep documentation and
  `system-knowhow/` in sync with the code they change.
- **Good citizenship**: helpful, respectful participation in issues, reviews,
  and discussions, consistent with the [Code of Conduct](CODE_OF_CONDUCT.md).
- **Reliability**: following through on what you take on, and reviewing others'
  work constructively.

The lead maintainer can then invite that person to become a maintainer and grant
the corresponding access.

### As the team grows

When the maintainer team outgrows single-leader decisions, this document will
describe the team-based model that replaces it. That may include a decision
quorum, an RFC process for major changes, and the scope of the lead maintainer's
tie-breaking role. Changes to this document follow the "significant decision"
process above.

## Amending this document

To propose a change to how Lucidos is governed, open an issue or a discussion.
The change is discussed in the open. Until the team-based model is in place, the
lead maintainer makes the final decision.
