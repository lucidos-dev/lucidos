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

The malicious probe proves hostile content executes inside its sandbox before
checking its authority. It also attempts return-grant changes, source operations
and return callbacks. Assertions check both refusal and absence of dispatch.
The minimal installed shell is not evidence for production-client continuity.

Run the production-client probe through its disposable engine harness:

```sh
./scripts/e2e-api.sh --local -f relay::tests::isolation::remote_production_client_continuity
```

This exercises Chromium and mobile WebKit with real conversations, reload,
trusted upgrade entry, storage preservation, SSE reconnect and Settings.
It also checks opaque app modules/fonts/SDK, trusted push workers and generation
advance. It uses the installed frontend rather than the minimal hostile fixture.

Run the separate real-engine routing topology:

```sh
./scripts/e2e-api.sh --local -f server::routing_topology_tests::isolated_directional_gateway_routing
```

This owns three gateways and engines, a private Postgres fixture, browser/device
credentials and CLI children. It never changes live pairings or grants.
The topology compares full messages, events, paging/counts, liveness and ladders
with target reads, and verifies new-thread acknowledgments and provenance.
It checks outgoing access without return grants, refused and granted return,
revocation, replay, stale discovery and source/routing/target process replacement.
The routing gateway is physically replaced; stale authoritative receipts fail
while fresh operations succeed. This covers I1/I4/I5/I7/I8/I9 alongside the
browser probes for I2/I3/I6.

Engine-contract router tests distinguish gateway support from running-engine
support, accept differing compatible builds, and reject missing, malformed,
oversized or incompatible health. Signed probes return only the contract and
recheck current support. Installed-client tests verify the installed client inventory,
contract generation, exact shell/asset hashes and missing-asset refusals.
