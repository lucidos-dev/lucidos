<!--
Thanks for contributing to Lucidos! Please fill in the sections below and the
checklist. Remember: there is no public CI — a maintainer can't tell whether your
tests pass unless you run them locally and say so here.
-->

## What this changes

<!-- A short description of the change and the motivation. What does it do, and
     why? -->

## Linked issue

<!-- Link the issue this addresses, e.g. "Closes #123". Open a PR without a linked
     issue only for trivial fixes (typos, obvious doc errors). -->

Closes #

## How it was tested

<!-- REQUIRED — there is no CI. List the suites you ran locally and the result,
     or explain why none were needed (e.g. docs-only / CSS-only). See
     CONTRIBUTING.md for which suites map to which changes. -->

- [ ] `make test` (Rust engine)
- [ ] `cd crates/lucidos-app && npx tsc --noEmit && npm test` (frontend)
- [ ] `./scripts/e2e-api.sh` (HTTP API surface)
- [ ] `./scripts/e2e-browser.sh` (UI behavior)
- [ ] `./scripts/e2e.sh` (full end-to-end)
- [ ] Not applicable — docs/CSS only

Details:

## Checklist

- [ ] My commits are **signed off** (`git commit -s` — DCO, see
      [CONTRIBUTING.md](../CONTRIBUTING.md#sign-your-work-dco)).
- [ ] My commit messages follow [Conventional Commits](https://www.conventionalcommits.org/)
      (`type(scope): summary`).
- [ ] I ran the relevant tests locally (above) and they pass.
- [ ] I updated documentation where needed — including the matching
      `system-knowhow/*.md` if I touched a documented surface (event type, SDK,
      CLI, plugin manifest, glossary term, …).
- [ ] I used the project's canonical vocabulary (workspace, app, intent, knowhow,
      trigger, event, artifact, …).
- [ ] This PR is focused on one logical change.
