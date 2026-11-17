# Diagnostic log

Shared diagnostic records for the engine, gateway and desktop client (ADRs
0716 and 0717). This crate has no engine dependency.

`line!` builds a typed log line. `log!` builds one and submits it to the
process's sink. The caller starts that sink with its own diagnostic directory
and registers known secrets through `sanitize::register_secret_values`.

```rust
use lucidos_diagnostic_log::{line, private, Destination};

let line = line!("request {id} failed: {}", private("private message"), id = 42_u64);
assert_eq!(line.render(Destination::Maintainer), "request 42 failed: <text 15 chars>");
```

Templates must be literals. Interpolated values implement `LogValue` or use
`private(value)`. Bare strings, paths and error messages fail compilation.
A private path uses debug formatting, or wraps the path's display adapter.
Custom kind enums implement `LogValue`; they must contain no private text.
`ErrorKind` separates an error's shareable kind from its private display.

Records retain each formatted fragment's class, alongside the original template.
The writer stores sanitized rendered text for existing query consumers.
Bundle builders use `writer::Record::render` for the destination, which ignores
that cached text when rendering maintainer output. Records without structured
fields render as `<unstructured line>` for the maintainer.

`install_panic_hook` records a private panic payload and preserves the previous
hook. Each process owns installing the hook and registering its secret values.
The engine retains its workspace-specific bounded query and authorization.
