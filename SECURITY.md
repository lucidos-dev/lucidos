# Security Policy

Thanks for helping keep Lucidos and its users safe. This document explains which
versions receive security fixes and how to report a vulnerability privately.

## Supported Versions

Lucidos is **pre-1.0** (currently the **0.9.x** line). While we're under 1.0, only
the **latest release** receives security fixes — there are no long-term support
branches yet, and older releases are not patched. Please reproduce on the latest
release before reporting.

| Version | Supported |
|---------|-----------|
| Latest release (0.9.x) | ✅ |
| Any earlier release | ❌ |

Once Lucidos reaches 1.0 this policy will be revisited and a longer support window
defined.

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues,
discussions, or pull requests.**

**Primary channel — GitHub private vulnerability reporting.** Report through
GitHub's built-in private reporting:

1. Go to the repository's **Security** tab:
   <https://github.com/lucidos-dev/lucidos/security>
2. Click **Report a vulnerability** to open a private advisory.
3. Describe the issue (see "What to include" below). Only you and the maintainers
   can see the report.

This is the preferred channel because it keeps the report confidential, gives us a
private space to discuss and fix the issue, and lets us coordinate disclosure and
credit through a GitHub security advisory.

> **Secondary channel (not yet available):** an email fallback at
> `security@lucidos.dev` is planned and will be added once the `lucidos.dev`
> domain and mailbox exist. **It does not work yet** — until then, use the GitHub
> private reporting path above.

### What to include

A good report helps us triage fast. Please include, as far as you can:

- The version / release (or commit) you reproduced on.
- The type of issue and the component or surface affected (engine, frontend,
  workspace data, an app, a trigger, the JS SDK, …).
- Step-by-step instructions to reproduce, including any proof-of-concept.
- The impact — what an attacker could do.
- Any suggested mitigation, if you have one.

## What to expect

- **Acknowledgement** that we received your report, as soon as we can.
- An initial assessment and, where relevant, a request for more detail.
- Coordination on a fix and a disclosure timeline. Because Lucidos is a small,
  pre-1.0 project with no public CI and no dedicated security team yet, please
  allow reasonable time for a fix before any public disclosure.
- **Credit** for the discovery in the advisory and release notes, unless you ask
  to remain anonymous.

## Scope

Lucidos is local-first: you own and run your own workspace, data, and LLM
credentials. Reports are most useful when they concern the engine, the frontend,
the way workspace data or credentials are handled, or the app/trigger/SDK
execution surfaces. Issues that require an attacker to already have full local
access to your machine, or that depend on a third-party service outside this
repository, may be out of scope — but when in doubt, report it and let us decide
together.
