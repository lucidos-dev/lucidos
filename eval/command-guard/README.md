# Command guard corpus

`cases.toml` is the single synthetic corpus for both tiers.
The engine's `command_permission::corpus_tests` uses the static classifier,
typed stub answers and the real action mapping for chat and triggers.
It runs in the engine suite without model credentials.

The floor controls also run across all four sandbox modes.

The primary replay models an enforced sandbox with truncation protection.
The B rows separately assert the text gate across all four sandbox modes,
with and without truncation protection. Where the sandbox owns the paths,
even B1 reaches the kernel without a card; the kernel still refuses its read.
These classifier tests do not replace the real confinement suite.

The model tier runs on demand:

```sh
cargo run --locked -p lucidos-engine-bin --features eval --bin lucidos-eval -- command-judge --models MODEL --out results.jsonl
```

It computes each marker with the real static pass under a synthetic `/workspace` root.
Rows with `model = false` exercise kernel or static decisions, which the model does not own.
Each result records the case id, marker, false positive, miss, error and token usage.
A reversible answer on a safe row counts as lane disagreement, but does not gate the command.
Errors remain visible and count as misses on irreversible rows.

The main check runs the model tier once on Linux when its environment has
`OPENAI_API_KEY` and the repository has `COMMAND_JUDGE_MODEL` as a variable.
The workflow remains secret-free; it does not provision credentials.
It uploads JSON lines with the engine suite evidence.
Without the key, it reports a skip; a skip supplies no model evidence.
