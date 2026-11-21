# 0520: Backups seal to an age backup recipient; the engine never holds the backup key

- **Status**: Accepted
- **Date**: 2026-10-18
- **Amends**: [0153](0153-state-dirs-opt-out-of-file-level-backup.md)
  (decision 3 and its "Show backup key" consequence)

## Context

Every archive was sealed with one symmetric AES-256 key, the workspace backup
key. The engine minted it, kept it in its `KeyStore` (ADR 0509), and revealed
it in Settings. So the machine that made a backup could always open it again.
Anyone who took over the machine, or a copy of its key store, could open every
archive at the destination too.

The `LUCIDOS1` format also had gaps. It sealed each chunk on its own, so it
missed a cut at a chunk boundary and a swapped pair of chunks. Its header was
not authenticated.

Plan: `docs/plans/2026-10-17-public-key-backups-and-1password-references.md`.

## Decision

1. **New archives are age files** (`.age`), sealed to one X25519 recipient per
   workspace, through the `age` crate.
2. **The engine stores only the public half**, the *backup recipient*, in the
   `backup_recipient` table. The private half, the *backup key*, is shown to the
   user once and never written anywhere.
3. **A minted key pair seals nothing until the user pastes its backup key
   back.** The engine derives the recipient from the paste, compares, and forgets
   the paste. A recipient the user brings themselves needs no such gate.
4. **No new legacy key is ever minted.** A workspace with neither a recipient
   nor a legacy key skips its scheduled backup with a "set up backups" notice.
5. **A legacy workspace keeps backing up with its old key until the switch.**
   The switch happens at the confirm step or at "Use my own age public key". It
   deletes the legacy key from the `KeyStore`.
6. **Only the owner at a registered device changes the recipient.** An agent
   that could set it would seal every later backup to a key it holds.

## Rationale

**After the switch, nothing on the machine can open a new backup.** That is the
property a backup needs and a symmetric key cannot give. It also suits a hosted
Lucidos: the operator holds only recipients and can never open a customer
archive.

**age is an open standard.** A user can open an archive with the `age` or `rage`
command line tool and no Lucidos at all. Its STREAM construction authenticates
the header, each 64 KiB chunk's position and the last chunk, which closes the
`LUCIDOS1` gaps.

**The paste-back gate exists because a lost key cannot be recovered.** It
follows Apple's recovery key and wallet seed phrases. Backups that run under a
key nobody saved are worse than no backups, because they look safe.

**Rotation is a second setup.** Settings offers Verify, Rotate and "Use my own
age public key", since the key can never be re-shown. Rotation re-enters the
same gate, so it needs no route of its own.

**Nothing pauses on its own during the switch.** A pending switch prompts the
user with a notice and a Settings banner, while the old key keeps sealing. A
workspace whose backups stopped because of a migration would be the worst
outcome of a security change.

## Consequences

- **We keep** the secrets key inside each archive (ADR 0509), now sealed to the
  recipient, so a restore still installs it.
- **We keep** every old archive readable. `LUCIDOS1` and `COGNOS01` stay as
  read-only decoders. The restore picker and `lucidos-engine restore-archive`
  read the archive header to pick the decoder. The key field takes either an
  age backup key or a legacy base64 key.
- **We give up** the "Show backup key" reveal for new workspaces. The reveal
  route survives only for a legacy key, so its owner can record it once more
  before the switch. ADR 0153's decision 3 is moot: no backup key is on the
  machine to place.
- **The user carries the backup key alone.** A yearly reminder suggests the
  Verify drill. Old `.enc` archives still need the old key after the switch,
  and the switch screen says so.
- A future workspace export reuses the same recipient, so one key opens
  everything.

## Alternatives considered

**Keep the symmetric key and protect it better.** Rejected. Any place the engine
can read the key from, an attacker on the machine can read it from too.

**A passphrase-derived key (scrypt).** Rejected. Scheduled backups would need
the passphrase on the machine, which recreates the symmetric case.

**Several recipients per workspace.** Deferred. One key per workspace keeps the
setup and the Verify drill simple, and age supports more later.

**age plugins for hardware keys.** Deferred. Each plugin is an external binary.

**Pause backups until the user switches.** Rejected, for the reason under
Rationale.
