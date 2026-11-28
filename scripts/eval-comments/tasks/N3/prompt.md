Auth handshake scripts only run once they are approved, so an edit over the
API cannot slip new code into a handshake. But I found a way around that.

I put an unapproved `sibling.py` next to an approved script under
`data/scripts/auth/`, and the approved script did `import sibling`. The
sibling's code ran, with the handshake's credentials in its environment. It
gets worse: a planted `data/scripts/auth/json.py` replaced the real `json`
module for every approved script there that imports `json`.

Only approved code should run with handshake credentials. A handshake script
should still be able to import the standard library and installed packages,
and still see its own path in `__file__`.

One more thing in the same area: when two approvals happen at the same time,
one of them sometimes disappears from the approval record without any error.
The script then refuses to run until it is approved again.
