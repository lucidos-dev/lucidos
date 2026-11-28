# 0615: Pairing secrets are long and never throttled; a typed 8-digit code is accepted only on a proved-local channel

- **Status**: Accepted
- **Date**: 2026-11-02

## Context

Device and gateway pairing each answered 10 wrong codes a minute for the
whole gateway. Once that budget was spent, even a correct code was refused
without a compare. A public address, such as a Cloudflare tunnel, let anyone
send 10 junk codes a minute. That locked every pairing out for as long as they
kept it up. No breach, but a denial of service for every user with a public
external address.

## Decision

Entropy is the defence. Nothing a flood can spend stands between a user and a
correct secret.

- Every pending pairing holds a 130-bit **pairing secret**: `ldp_` for a
  device, `lgp_` for a gateway, then 26 base32 characters. It is accepted on
  every channel and never throttled. The QR, the pairing link and the desktop
  self-pair carry it. A gateway pairing has nothing else.
- A device pairing also offers the 8-digit **pairing code** for a person who
  must type. The gateway accepts it only on a **proved-local channel**, behind
  a per-source budget with a global backstop above it. A public request is
  refused before any compare and counts against nothing.
- A request is local only when all three hold:
  - It arrived on a listener not marked public.
  - It carries no tunnel or proxy header.
  - Its socket peer is a private LAN address, or reached an address of this
    machine that Tailscale reported. A caller on this machine never counts: a
    loopback peer, or one whose address is the local address it reached.
- The gateway runs a loopback **public listener** (its port plus 30). A tunnel
  or reverse proxy points there, and every request on it is public.
- With nothing pending, a redemption is refused at once and counts nothing.

## Rationale

A per-source budget alone only moves the problem: a global backstop brings the
lockout back for anyone with many addresses. Dropping the backstop does not
work either, since 8 digits against 1,000 addresses leaks a real chance. A
130-bit secret needs no budget at all. So the short code survives only where
a flood cannot reach it: another device on the user's LAN or tailnet.

A loopback peer proves nothing. cloudflared, `tailscale serve`, `ssh -L` and a
raw TCP forward all connect from 127.0.0.1. A raw forward adds no header and
passes whatever `Host` its caller sends. So no check can tell it from a local
browser. A browser on this machine has the link instead, and a
header can only make a request stricter.

## Consequences

- A typed code stops working through any tunnel or proxy, including
  `tailscale serve`. The QR, the link and a pasted secret still work there, and
  the refusal says so.
- Gateway pairing is pasted, never typed. A newer gateway still accepts an older
  gateway's 8-digit code when adding it as a remote. The reverse fails: an older
  gateway wants 8 digits and refuses a secret before it sends anything, so it
  must update first. The settings screen that mints the secret says so.
- Some direct callers cannot be proved local, and their typed code is refused.
  The QR, the link and a pasted secret still work for each:
  - a LAN peer on a global IPv6 address;
  - a tailnet peer whose address changed since the daily certificate refresh
    read the tailnet addresses.
- A browser on the machine Lucidos runs on cannot type the code. It opens the
  link `lucidos pair` prints, and the desktop app pairs itself with the secret.
- A tailnet peer through tailscaled in userspace-networking mode connects from
  127.0.0.1, so its typed code is refused too.
- A forwarder on another LAN host that rewrites the source address, such as
  hairpin NAT, reads as a LAN peer. Its callers get the throttled typed code.
- Each tailnet peer keeps its own short-code budget, because the control server
  assigns its address. Other IPv6 sources share one budget per /64.
- A trusted-proxy list to key the budget on a forwarded address is not built.
  Any forwarding header already makes a request public, so the budget never
  sees one.

## Alternatives considered

- **A per-source budget with a global backstop**: the backstop is the lockout,
  for anyone with many source addresses.
- **A per-source budget with no backstop**: a distributed guesser's odds grow
  with the number of addresses.
- **Trusting the loopback peer**: every local tunnel would read as local.
- **Trusting a loopback peer with a loopback `Host`**: a raw TCP forward lets
  its caller send `Host: localhost`, so a stranger got the typed code.
- **Longer typed codes**: still guessable at scale, and worse to type than a
  QR or a paste.
