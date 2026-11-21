# 0728: Each command guard harm has one owner

- **Status**: Accepted
- **Date**: 2026-11-17

## Context

Repeated false positives came from layers deciding facts they did not own.
Static analysis conflated scratch writes with outside destruction.
Secret-path text matches prompted even when the sandbox would refuse the operation after approval.
The guard needs one owner for each harm and a standing regression corpus.

## Decision

1. The sandbox owns secret reads where it enforces reads and truncation.
   The text gate remains elsewhere.
2. The outside marker represents evidence of an outside target.
   Scratch targets and unresolved links do not set it.
3. The guard and write confinement share one definition of OS temp directories.
4. A command asks only below 0.7 on P(safe) + P(reversible).
   Below 0.7 on safe alone, it checkpoints and runs.
5. The judge reads bounded workspace script source, never outside or hidden paths.
   Script content participates in the cache key.
6. The fast path proves Safe from allowlists.
   Unknown flags, subcommands and Python imports go to the judge.
7. The catastrophic floor reads operations, never data written by a data sink.
8. This decision absorbs the earlier split-probability plan's logging and threshold changes.
9. An unknown script invoked outside the workspace still needs a grant.
10. Private paths deny agent reads and writes through the sandbox.
    In-process file tools check the same list; the built-in private subtree remains mandatory.
11. Only Settings user actions edit private paths.
    Machine entries live in `~/.lucidos/private/private-paths.toml`; workspace permissions can add entries, never subtract them.
    Entries name exact files or folders; no tool, CLI or permission card bypasses them.
12. Always deny writes a standing command rule with the same scope as Always allow.
    Denies beat all allows on every lane and remain removable in Settings.
    Command rules do not protect files.
13. A gated command fails a trigger run with one permission card to grant and run again.
    The notification opens that card; its banner carries no grant button.
14. Trigger grants allow a program, using the same scope as Always allow.
    Side-effect categories retire.
15. Category grants end on upgrade. The owner opens **Review trigger grants**
    from the conditional release notice. Each affected workspace gets one
    durable review thread. Each card lists the programs from one trigger's
    earlier runs that a grant could allow.

    All programs start checked.
    **Save grants** saves only checked programs. Clearing every checkbox
    answers the card without saving grants.

    Startup recovers interrupted answers on an existing review.
16. Trigger subprocesses keep static scope checks ahead of execution in all modes.
    Enforced Seatbelt and Landlock ABI 3 or later restrict kernel-mediated writes to the workspace,
    OS temp directories and revalidated folders granted to that trigger.
    Private paths, standing denies and the catastrophic floor retain precedence.
    Chat and coding-agent execution keep their existing policy.

    Failed subprocess output with an absolute path or the macOS Seatbelt log can reveal a refusal.
    For an eligible folder, the run fails with a permission request to grant it and run again.
    A hidden refusal in a process that exits successfully can stay silent.
    Capturing a refusal from a descendant or detached process is best effort.

    Linux matches Claude Code's sandbox runtime for triggers.
    The trigger child unshares a user and mount namespace and mounts everything outside its write scope read-only.
    Landlock then applies on top, so contents, entries and metadata outside the scope are all refused.
    The sandbox crate does this itself with `open_tree`, `mount_setattr` and `move_mount`: no bubblewrap dependency.

    A probe decides the mode once per engine.
    Where the kernel refuses the namespace, the trigger runs under Landlock alone, and Settings reports the metadata gap.
    Once the probe passes, a namespace failure fails the spawn, so the reported mode stays true.
17. Trigger details lists each folder grant beside the program grants, revocable one at a time.
    Only the owner revokes a folder grant, unlike the coding-agent grant of ADR 0641, which anyone may revoke.
    The owner chose where the trigger writes after seeing the folder on a permission card.
    An agent's revoke would fail the next run and ask the owner the same question again.

## Rationale

The kernel enforces private access and confined writes across subprocesses.
Static analysis owns filesystem location and the catastrophic floor.
The model owns real-world side effects, with code applying one threshold.
Unknown syntax remains uncertainty rather than a proof of safety.
Each decision has deterministic regression coverage in `eval/command-guard/cases.toml`.
A manual model tier consumes the same rows, using the marker computed by static analysis.

## Consequences

The additional trigger write scope applies on macOS and on Linux with Landlock ABI 3 or later.
Seatbelt and the Linux read-only mounts refuse metadata operations such as chmod, chown, setxattr and utime.
Landlock alone mediates only contents, truncation and directory entries, so the fallback leaves metadata open.
Neither system closes hard links: a writable name that shares an outside file's contents still writes them.
Inside the namespace, files owned by other users show as `nobody`.
Sandbox off, inherited sandbox and Linux below ABI 3 retain the static scope check without the
additional trigger restriction.
Older Linux kernels retain text gates because read protection cannot stop truncation.
Opted-out and inherited sandboxes retain static checks and disclose the missing enforcement in Settings.
Running sessions retain their initial policy until their next start.
Each Lucidos home owns its machine list; workspace additions cannot weaken it.
On macOS, the engine reads Seatbelt denials from a live `log stream`, which needs an admin account.
For a standard user, only a refused path in the command's output offers a folder grant.

The corpus contains synthetic commands only.
It records baseline failures explicitly while their implementation phases remain incomplete.
The model check reports each false positive and miss; deterministic tests require no credentials.
A Python program grant covers all Python for that trigger.

## Alternatives considered

- More substring exceptions keep location and access policy in the wrong layer.
- Grant buttons in notifications lack enough context and do not render on every device.
- Deny-and-continue hides partial trigger failures; parked runs act on stale data.
- Private-path globs and one-off bypass cards weaken a boundary the user expects to hold.
- Confining chat writes was offered and rejected; chat retains static scope checks.
- Bubblewrap would add a runtime dependency that packaged installs do not ship; the same syscalls need no binary.
- Silently dropping to Landlock alone after a namespace failure would misreport the mode in Settings.
- A `log show` after the command exits finds no denial: the kernel streams Seatbelt denials but does not store them.
