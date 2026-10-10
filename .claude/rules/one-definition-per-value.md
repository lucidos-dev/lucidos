# One Definition Per Value

**Always loaded**: a second copy is written before any file names it.

- **Write a value once.** A default, limit, id list or name shared by two
  places lives in one, and the other reads it.
- **Readers resolve defaults from the source.** A preference goes through its
  `prefs::` handle (Rust) or the generated catalog (TS). Never pass a fallback.
- **Another language gets a generated copy**, with a staleness test. A pin test
  only where import or generation cannot reach (shell, raw docs).
- **Tests reference, never restate.** A literal only pins a wire contract,
  and says so.
- **Same number, different concept, stays apart.** Name both, don't merge.

Enforcement per class, and what was rejected: ADR 0368.
