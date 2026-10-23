A Python script run by the agent appended one line to an existing file,
`data/artifacts/log.jsonl`, with `open(path, 'a')`. The run reported success,
but afterwards the file held only the new line. The 500 lines it had before
were gone.

Opening an existing file under `data/` with `'r+'` also fails with
`FileNotFoundError`, although the file is plainly there. Writing with `'w'`
works as expected.
