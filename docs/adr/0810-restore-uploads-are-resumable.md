# 0810: A backup restore uploads in resumable tus 1.0 chunks, then commits

- **Status**: Accepted
- **Date**: 2026-11-25
- **Amends**: [0015](0015-restore-in-the-workspace-picker.md)

## Context

ADR 0015 restores a backup from one multipart request that carries the whole
archive. A 1.77 GB archive sent to a gateway behind a Cloudflare tunnel never
arrived: Cloudflare caps a request body at 100 MB on standard plans. The picker
had no progress, timeout or 413 handling, so its button read "Starting..."
forever. nginx caps a body at 1 MiB by default, and corporate proxies vary.

A restore onto a remote gateway (ADR 0811) crosses that gateway's proxy too.

## Decision

1. **A restore uploads first and commits second.** The archive arrives as a
   *restore upload*: a resumable upload session on the gateway, written in
   chunks. `POST /~/api/v1/control/workspaces/restore` then takes JSON
   `{upload, key, name}` and starts the restore. The multipart form is gone.
2. **The upload wire is a subset of tus 1.0**: core, creation, checksum
   (`sha256`), expiration and termination.
   - `POST /~/api/v1/control/restore-uploads` with `Upload-Length` and
     `Upload-Metadata` creates a session and answers `201` with a `Location`.
   - `HEAD <location>` answers `Upload-Offset` and `Upload-Length`.
   - `PATCH <location>` with `Upload-Offset` and
     `Content-Type: application/offset+octet-stream` appends one chunk.
     `Upload-Checksum: sha256 <base64>` is verified when present.
   - `DELETE <location>` ends the session and removes its bytes.
3. **The client starts at 32 MiB chunks and halves on a 413**, down to a
   512 KiB floor. Below the floor it names the proxy's limit as the error.
4. **A session lives on disk** under the gateway's config directory, with its
   offset beside its bytes, owner-only. A gateway restart keeps it.
5. **Sessions are bounded.** Four at once, and 24 hours after the last chunk a
   sweep removes them. Creating one refuses when the disk lacks about three
   times the archive size, which the restore later needs for decrypting and
   unpacking.
6. **The commit refuses an incomplete upload.** A `PATCH` at the wrong offset
   answers 409, and a checksum mismatch answers 460 and keeps nothing.
7. **The backup key travels only in the commit body**, then in the engine's
   environment as before. It never reaches the session's files.

## Rationale

- **No single request carries the archive**, so the size of the archive and
  the body cap of the proxy stop interacting. Halving on 413 reaches any cap
  above the floor with no configuration.
- **tus is an open standard** (philosophy rule 3). The IETF resumable-upload
  draft descends from it but is not final. The subset is small enough to
  implement on both ends without a dependency.
- **Resume needs the offset to be the gateway's**, not the client's. The
  client asks with `HEAD` and continues from there, after a drop, a reload, a
  sleeping laptop or a gateway restart.
- **Integrity has two layers.** A per-chunk SHA-256 catches a corrupted chunk
  before it is kept, when the browser can hash (`crypto.subtle` needs a secure
  context). The archive's own authenticated encryption (age STREAM, or the
  legacy AES-GCM chunks) catches anything else when the engine decrypts the
  whole archive before `pg_restore` starts.

## Consequences

- The picker shows upload progress, then the restore phases.
- A page on a non-secure origin sends no chunk checksum. The length check and
  the archive's authentication still hold.
- Chat blobs, data uploads and plugin archives remain single requests. They
  show a plain error for a 413 instead of a generic one.
- The staged bytes cost disk for up to 24 hours when an upload is abandoned.

## Alternatives considered

- **Raise the proxy's limit.** Not ours to raise: Cloudflare's cap is a plan
  limit, and a corporate proxy belongs to the employer.
- **Restore straight from Dropbox or Google Drive on the gateway.** ADR 0015
  rejected it: the gateway holds no provider tokens. Chunked upload covers
  every size without it.
- **Restore from a file already on the gateway host.** Useful headless, but a
  second entry point that a phone or a remote desktop cannot use.
- **A private chunk protocol.** Rule 3: a standard exists.
- **A whole-file hash in the browser.** `crypto.subtle.digest` does not stream,
  so a 2 GB file would need to fit in memory or a hashing dependency. The
  archive already authenticates itself.
- **Keep the multipart route beside the new one.** Two paths to keep honest,
  and the gateway serves its own picker, so no older client needs it.
