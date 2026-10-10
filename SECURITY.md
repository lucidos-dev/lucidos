# Security Policy

This document lists which versions receive security fixes and how to report a
vulnerability privately.

## Supported Versions

Lucidos is **pre-1.0**. Only the **latest release** receives security fixes.
There are no long-term support branches. The latest release is the newest `v*`
tag on [GitHub Releases](https://github.com/lucidos-dev/lucidos/releases).
Please reproduce there before reporting.

| Version | Supported |
|---------|-----------|
| Latest release | ✅ |
| Any earlier release | ❌ |

At 1.0 we will revisit this policy and define a longer support window.

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues,
discussions, or pull requests.**

**Primary channel: GitHub private vulnerability reporting.** Report through
GitHub's built-in private reporting:

1. Go to the repository's **Security** tab:
   <https://github.com/lucidos-dev/lucidos/security>
2. Click **Report a vulnerability** to open a private advisory.
3. Describe the issue (see "What to include" below). Only you and the
   maintainers can see the report.

We discuss and fix the issue in that private advisory. We coordinate disclosure
and credit through it as a GitHub security advisory.

> **There is no email channel.** Use GitHub private reporting above. Do not rely
> on `security@lucidos.dev` until this page lists it as a working fallback.

### What to include

Include as much of this as you can:

- The version / release (or commit) you reproduced on.
- The type of issue and the affected component or surface (engine, frontend,
  workspace data, an app, a trigger, the JS SDK, …).
- Steps to reproduce, including any proof-of-concept.
- The impact: what an attacker could do.
- Any suggested mitigation.

## What to expect

- **Acknowledgement** that we received your report, as soon as we can.
- An initial assessment and, where relevant, a request for more detail.
- Coordination on a fix and a disclosure timeline. Lucidos is a small, pre-1.0
  project with no dedicated security team yet. Please allow reasonable time for
  a fix before any public disclosure.
- **Credit** in the advisory and release notes, unless you ask to remain
  anonymous.

## Scope

Lucidos is local-first: you own and run your own workspace, data, and LLM
credentials. The most useful reports concern the engine, the frontend, the
handling of workspace data or credentials, or the app/trigger/SDK execution
surfaces. An issue may be out of scope if an attacker must already have full
local access to your machine. An issue that depends on a third-party service
outside this repository may also be out of scope. When in doubt, report it and
we will decide together.
