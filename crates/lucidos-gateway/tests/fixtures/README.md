# Malicious paired-workspace fixtures

These fixtures run behind the real gateway relay and engine-socket proxy in
`src/relay_isolation_tests.rs`. All gateway registries, credentials, listeners,
browser profiles and storage are disposable. There is no dependency on a live
workspace or a machine's pairing state.

Run the router regressions:

```sh
cargo test --locked -p lucidos-gateway relay::tests::isolation:: -- --nocapture
```

Run the actual browser probe (both Chromium and mobile WebKit, provisioned
Playwright required):

```sh
cargo test --locked -p lucidos-gateway remote_browser_origin_boundary -- --ignored --nocapture
```

The browser test is explicit because ordinary gateway unit tests do not require
Node or installed browsers. The full isolation topology verification must invoke
it, not infer a browser pass from the default Rust suite's ignored-test count.

| Plan invariants | Probe | Required result |
| --- | --- | --- |
| I2/I8 | B supplies a hostile shell while A has its own installed shell | A client or explicit upgrade refusal, never B's shell |
| I2/I3 | Direct-open API document reads A storage and private workspace list | Opaque enforcing sandbox; both reads fail |
| I2/I6 | Foreign JavaScript, worker registration and missing asset | No foreign executable served as trusted code |
| I3/I9 | B redirects into A/C, including relative/encoded paths | Refused before private data can be fetched |
| I2/I6 | Browser cache write, worker install, subsequent remote navigation | No origin cache write or foreign worker controller |
| I1/I7/I9 | Existing `relay_tests.rs` directionality, cookies, revoke, streaming tests | Preserve explicit signed outgoing access and live streams |

Phase 1 recorded failing isolation assertions against the old runtime.
The first phase-2 slice makes those assertions pass. Additional router tests
cover A's installed assets, refused missing assets, safe redirects, content
mounts and foreign-source refusal before dispatch. This remains an intermediate
checkpoint, with the rest of the approved plan still to implement.

The browser probe proves hostile content actually executes before testing its
authority; refusing all documents cannot masquerade as successful opaque app
rendering. Later verification needs the real frontend, a pre-upgrade worker,
legitimate apps/bridges/SSE and the A/B/C agent topology.
The minimal installed shell here is not
evidence for the production frontend upgrade path.

Engine-contract router tests now distinguish gateway support from running-engine
support, accept differing compatible builds, and reject missing, malformed,
oversized or incompatible health. Signed probes return only the contract and
recheck current support. Installed-client tests verify the installed client inventory,
contract generation, exact shell/asset hashes and missing-asset refusals.
