Agents get a database URL for the workspace, and it is meant to be safe to hand
out. But an agent that opens a read-write transaction on it can still change
things only I should control.

I tried it with a coding agent. Inside `BEGIN READ WRITE` it set the
`snapshot_restore_by_agents` preference to `allow`, which lets agents restore
snapshots without asking me. It could also change other preferences the
settings screen keeps from agents. It could even delete the row in the
snapshot restore boundary table. As I understand it, that makes a permission I
granted usable again after a restore expired it.

Separately: when I restore an old snapshot, the restore policy from that old
snapshot comes back with it. If the snapshot was taken while restores were
set to `allow`, restoring it turns `allow` back on, even though I changed it
since.

The agent's database credential should not be able to change those
preferences or the restore boundary. Ordinary tables and ordinary
preferences should stay writable for it, as they are now. A restore should
keep the restore policy I have today.
