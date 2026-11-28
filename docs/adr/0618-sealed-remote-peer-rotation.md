# 0618: Seal origin-bound remote headers and recover credential rotation from durable sender intent

- **Status:** Accepted
- **Date:** 2026-11-02

## Context

Remote gateways can sit behind an access proxy that requires secret headers.
Persisting those headers or gateway credentials as plaintext exposes them to
file readers. Changing a peer credential also crosses two independent durable
stores: accepting a new credential before the caller saves it can strand an
unattended pairing after a lost response or a crash.

## Decision

Share the secret-store implementation between engine and gateway. Seal remote
credentials and proxy header values with context bound to the pairing and the
field name. Refuse unreadable secrets without replacing their key. Upgrade
legacy plaintext credentials atomically before publishing the loaded store.

The packaged macOS app uses Keychain. Other installs use an owner-only key file: this
protects a copied ciphertext file. It cannot protect against someone who
can read both key and ciphertext as the same OS user.

All outbound peer calls use one HTTPS transport that refuses redirects. When
headers are configured, bind them to the origin the user entered; do not even
probe an advertised alternative with those secrets. Header values are
write-only in management APIs. Header edits replace the complete set. Reserved
transport and Lucidos header names cannot be supplied. Browser relay requests
cannot override the configured proxy headers.

For credential rotation, A securely generates a 32-byte candidate and seals a
durable intent before sending it to B. The candidate has a distinct sealing
context from the current credential. B stores only digests, accepts the same
candidate idempotently, and retains the previous digest for at most ten
minutes or until the candidate is used. The previous credential cannot start
a different rotation during overlap.

A first authenticates with its pending candidate. A 401 permits retrying the
installation with the old credential and the same candidate. A confirms the
pairing identity with the candidate before replacing its current credential.
Failures retain both secrets and a visible error. This recovers after either
process restarts, even if B accepted but A lost the response and the overlap
expired. B minting a replacement and returning it was rejected because it
cannot recover that failure without another durable exchange.

Rotate every 30 days and on demand. Recover pending work on startup and retry
hourly. Serialize sender rotation, header replacement, and removal. Removal
tries a pending candidate before the old credential. Older peers remain
usable and report unsupported rotation visibly; retry allows a later peer
upgrade to complete the same intent. Device credentials do not rotate here.

## Consequences

This covers macOS and Linux gateway installations, on tailnets and behind
user-run HTTPS access proxies, without provider-specific transport code.
Secret recovery requires the original sealing key. A hard credential expiry
is avoided so an offline remote remains recoverable. Unpair remains explicit
revocation. Pending candidates never appear in management responses or Debug
output. The gateway protocol advances to version 2 while version 1 peers can
still pair and relay.
