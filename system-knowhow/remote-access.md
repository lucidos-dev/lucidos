---
name: Remote Access & HTTPS
description: Use to reach Lucidos from a phone, tablet, another machine or a Kubernetes pod. Covers "remote access", "Mobile Access", "Expose", "tailscale serve", "tailscale funnel", "expose a webhook". Also "HTTPS", "not secure warning", "add to home screen", "certificate", "mkcert".
---

# Remote Access & HTTPS

Reaching a Lucidos install from a phone, a tablet, or a second computer. Two
knobs decide whether it works, and they are **independent**:

1. **Where the gateway listens** (the *network bind*): loopback only by default,
   so nothing off this machine can connect directly.
2. **Whether the browser sees a secure origin** (https, or localhost): this
   decides the "Not Secure" label, and whether service workers, web push and
   PWA install work at all.

Most confusion comes from mixing them up. A perfect tunnel still shows "Not
Secure" over `http://`. A perfect certificate is useless if the gateway binds
loopback and nothing proxies to it.

## Parts

This doc is split into parts so each one fits a coding agent's tool output. Load the part that covers what you need.

| Part | Covers |
|---|---|
| `system-knowhow/remote-access/diagnose-first-never-assume` | Diagnose first, never assume (Step 1: who is listening, and on what address; Step 2: which port speaks TLS to other devices; Step 3: repeat from the remote device's viewpoint); Devices pair before Lucidos answers them (Public only behind a front door you own; The one exception: a webhook's own socket); Why HTTPS matters, and when it does not |
| `system-knowhow/remote-access/the-three-routes` | The three routes (Automatic: the gateway's own certificate; Route A: plain HTTP over the Tailscale tunnel; Route B: `tailscale serve` (recommended); Route C: mkcert (Lucidos terminates TLS itself); A second laptop: open it through its own Lucidos); The local certificate: `https://localhost` on the gateway's machine; Any other VPN |
| `system-knowhow/remote-access/settings-access` | Settings → Access (Pair a device; A phone installs before it pairs; The list of devices lives in Settings → Devices; Users: whose device is which) |
| `system-knowhow/remote-access/settings-access-2` | Settings → Access (continued) (Pair a gateway; What a device is called; The tailnet-status endpoint) |
| `system-knowhow/remote-access/settings-access-3` | Settings → Access (continued) (The Expose run); The detection trap: Tailscale state does not tell you whether HTTPS works; Network bind (In a container (Kubernetes, Docker); Restarting the gateway; An engine that faces a network asks for a credential); Connecting accounts from another device (Sign in with ChatGPT on a headless workspace; Claude Code's MCP login is a different flow); Operational notes; Quick triage; Diagnostic evidence |
