# 0031: A deploy to a lucidos.dev origin runs on the maintainer's machine, never from public CI

- **Status**: Accepted
- **Date**: 2026-07-31

## Context

Two origins are published out of one Cloudflare account, and until today they
were published two different ways.

`lucidos.dev` itself has always been deployed from the maintainer's machine, off
a workspace trigger chain: `LucidosReleased`, then the Download-for-Mac link
bump, then `SitePublishRequested`, then the publisher, then `SitePublished`.
Nothing in `.github/workflows/` touches it. That is precisely why
`install-smoke.yml`'s post-publish front-door run is a `workflow_dispatch` fired
*by the publisher* rather than the `release: published` webhook: the webhook
arrives while the deploy is still in flight and would verify the previous
origin, passing for the wrong reason.

`docs.lucidos.dev` was the exception. `.github/workflows/docs.yml` ran
`mkdocs build --strict` on the public mirror and deployed `site/` to the
`lucidos-docs` Pages project with `cloudflare/wrangler-action@v3`, reading a repo
secret `CLOUDFLARE_API_TOKEN`.

**That secret does not exist on the mirror and never did.** The repo has
`CLOUDFLARE_ACCOUNT_ID`, `RC_ACCESS_CLIENT_ID` and `RC_ACCESS_CLIENT_SECRET`, and
nothing else. So the job went red on every release from 2026-07-11 onward (runs
on 07-11, 07-29 twice, 07-30 and 07-31), each one dying at the deploy step on:

```
In a non-interactive environment, it's necessary to set a CLOUDFLARE_API_TOKEN environment variable
```

The visible cost was twenty days of stale documentation: `docs.lucidos.dev` went
on telling readers "No prebuilt release is published yet, so the default download
will 404 today" across every release in that window, each of which published one.
The structural cost was worse. A job that is red on every single release is a job everyone learns to
scroll past, and it takes the honest reds down with it.

The obvious fix, adding the secret, is the one we rejected.

## Decision

**A deploy to any lucidos.dev origin runs on the maintainer's machine, off a
workspace trigger. `.github/workflows/` deploys nothing.**

`.github/workflows/docs.yml` is deleted. Docs publishing is the workspace trigger
"Publish lucidos.dev docs": it fires on `LucidosReleased` (or an explicit
`DocsPublishRequested`), installs `requirements-docs.txt` into a dedicated venv,
runs `mkdocs build --strict` as the gate, deploys `site/` to the `lucidos-docs`
Pages project through the engine's API proxy, fetches routes back from that
deployment's own preview URL before it will call the publish a success, and emits
`DocsPublished` or `DocsPublishFailed`. It published the 0.18.0 tree at 06:02 on
2026-07-31.

That is the same shape as the site publisher, deliberately. The rule now covers
both origins and has no exception.

## Rationale

**The credential decides it.** A Cloudflare token that can deploy Pages for this
account also carries, in the form available here, `dns_records:edit` and
`zone:edit` on the `lucidos.dev` zone. That is a credential which can repoint the
zone, parked in a *public* repository's Actions environment, so that a deploy can
happen in the one place that has no need to be doing it. Write access to a
workflow file is enough to make a job hand over a secret it is entitled to, and
the mirror's threat model should not extend to "and then the attacker owns DNS
for the domain the installer is curled from". The blast radius is out of all
proportion to the task, which is uploading a static site.

**The deploy has no reason to be in CI in the first place.** Nothing about it
needs a hosted runner: the maintainer's machine already holds the release
checkout it cuts the release from, and the build is `pip install` plus `mkdocs`.
CI was not buying automation the machine could not do, it was buying automation
in exchange for holding the key.

**Verification travels with the deploy.** The workspace publisher fetches routes
back from the deployment's own preview URL and only then reports success, which
is the check that actually distinguishes "the upload returned 200" from "the site
serves the new pages". CI cannot verify the live custom domain right after its own
deploy for the same reason the front-door check is a publisher-fired dispatch: a
webhook races the propagation. The lesson is already paid for. On 2026-07-29 a
Pages deploy published `install.sh` but not the helper libs beside it, and because
Pages soft-404s (landing-page HTML at status **200**) the advertised one-liner
sourced HTML as shell. Post-deploy verification is where that class of bug is
caught, and it belongs next to the deploy.

**One rule beats a rule with an exception.** With the site published one way and
the docs the other, the exception was carrying the whole cost: the broad
credential existed in CI for exactly one of the two, and the mismatch is what let
a permanently broken path go twenty days without anyone noticing it was the *only*
path publishing that way.

**A permanently red job is a liability, not a dormant asset.** It cost the
project a stale docs site, and it spent the credibility of every other red mark in
the same tab. Deleting it is not losing coverage. The coverage was never there.

## Consequences

- **`.github/workflows/` is now verification only, with nothing that deploys.**
  Two files remain, `install-smoke.yml` and `release-tarballs.yml`, and both test
  a tree, an artifact, or a live origin. No workflow needs a Cloudflare
  credential; `CLOUDFLARE_ACCOUNT_ID` is now referenced by none of them. This
  sharpens the standing "GitHub Actions is release-only" rule in `CLAUDE.md` and
  `.claude/rules/build-release.md`: release and delivery **verification** only.
- **The gate is unchanged, it just runs elsewhere.** `mkdocs.yml`, `docs/site/`
  and `requirements-docs.txt` all stay, and `mkdocs build --strict` is still what
  stops a broken transclusion or a dead link from shipping. The standing
  constraint from the 2026-07-30 docs audit still bites (links inside the
  transcluded blocks must be absolute `github.com` URLs, or the strict build
  fails), it now fails the trigger rather than the workflow.
- **A docs publish needs the maintainer's machine up and the workspace running.**
  A release cut while it is not leaves the docs stale until the trigger fires or
  someone emits `DocsPublishRequested` by hand. That is a genuine loss of
  unattended-ness, accepted because the failure is now loud where the maintainer
  actually looks: a `DocsPublishFailed` event and its thread, rather than a red
  tab on a public repo nobody opens after a release.
- **Re-adding a deploy workflow re-opens this decision.** Including a "just for
  docs, just this once" one, which is the exact shape of what we removed.

## Alternatives considered

- **Add `CLOUDFLARE_API_TOKEN` to the mirror.** The one-line fix, and the reason
  this ADR exists. Rejected: it puts a token that can edit DNS and the zone into a
  public repo's CI in order to automate something that was already automated
  elsewhere. The cost is a permanent, standing risk; the benefit is a deploy step
  moving to a machine with no advantage over the one it left.
- **Mint a narrower, Pages-only token.** The honest version of the above.
  Rejected twice over: the token form available for this account carries the zone
  scopes anyway, and even a perfectly scoped one would still leave the deploy in
  the wrong place, unable to verify the origin it just published and duplicating a
  publishing path that already existed and worked.
- **Keep the workflow but stop it going red**, via `continue-on-error: true` or a
  guard on the secret being present. Rejected: a job that cannot fail is not a
  gate, and one that skips silently is indistinguishable from one that worked. The
  mirror would carry a docs deploy that never deploys, which is strictly worse
  than carrying nothing, because the next person reads the filename and believes
  it.
- **Fold docs into the existing site publisher, one deploy for both origins.**
  Tempting, since it is one credential and one code path. Rejected: separate Pages
  projects, separate custom domains, separate build inputs, and a genuinely
  independent cadence (a docs correction should republish without touching the
  landing page's download links, and a DMG-link bump should not rebuild the docs).
  Two triggers, one rule.
