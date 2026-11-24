# Client probe pass

## Scope and result

This pass covers every shipped client: iOS Safari/PWA, macOS WKWebView,
desktop browsers and SDK frames. It inspects the tree and recorded registry
findings, without reading workspace user data. No probe has affirmative evidence
that its full removal condition is met. No source or scroll file changes.

Costs below are source-level counts and cadence, not CPU, memory or latency
benchmarks. Device verification, fresh diagnostic audits and release-counter
measurements were not performed. None of these diagnostic conditions is gated
on a release counter.

## Temporary probes

Paths below are relative to `crates/lucidos-app/src/` unless stated otherwise.
All rows are **kept**. The registry preserves their individual removal steps.

| Probe and location | Positive evidence checked; why retained | Runtime cost |
|---|---|---|
| Focus reveal: `hooks/useFocusedFieldVisible.ts::reportEpisode` | `ios-keyboard-reveal-placement` remains open; no recorded passing device run. | One geometry payload per armed settle episode; bookkeeping on existing focus/viewport work. |
| Page health: `utils/pageHealthLog*` | Registry still requires both editing and crash investigations to close; no closing audit. | Touch counters wrap listener/timer/observer APIs; 500 ms lag interval and 30 s visible report with frame-rate sampling and DOM/resource reads. |
| Crash resources: `utils/crashMonitor.ts`, `crashBreadcrumbs.ts`, `crashStorage.ts`, `pageResources.ts` | Shared page-health row still active; death classes lack recorded closure. | Resource observer and load listener; visible sampling normally 10 s, 2 s for 20 s after thread open, 500 ms for 15 s after wake; bounded persisted samples/marks. |
| SDK frame resources: `packages/lucidos-sdk/src/widgetResources.ts`, host `utils/frameResources.ts` | Already registered under Page health log; shared crash investigation remains open. | Resource-change media/image counting, messages to host and mounted-frame aggregation; no resource paths. |
| App frame load: `components/apps/appFrameLoadProbe.ts` | Six recorded desktop episodes; admission stage unidentified, no phone fix verification. | One 3 s timer per mounted frame; slow frames log start and end/abandon, fast frames silent. Shared request/queue snapshots add bookkeeping. |
| Shell snap-back: `utils/shellScrollProbe.ts` | No recorded source-trigger fix and packaged-macOS verification. | Geometry/focus payload on guard correction; at most one log per second, one coalescing timer. Permanent guard stays. |
| Safe-area: `utils/safeAreaProbe.ts` | ADRs 0551/0553/0610 describe fixes and confirmation goals, not a recorded audit closing both goals. | Geometry/style/layout reads on changed inset state; deferred log; 3 s settle timer retries at most 20 times. |
| Toast press: `components/shared/toastPressProbe.ts` | No recorded first-tap cause and verified fix. | Five passive document listeners; per-press mutation observer and deadline/grace timers, verdict log. |
| Composer press: `components/chat/deadPressProbe.ts` | ADR 0262 explains autocorrect, but later recorded touch-delivery and cancelled-tap episodes extend the diagnosis. No closing audit. | Passive touch/click/input listeners, per-press observer and deadline/grace timers; 3 s scheduled checks and gated keyboard polling/relayout support. |
| Composer typing: `components/chat/deadKeystrokeProbe.ts` | Registry explicitly says autocorrect does not explain missing characters. | Two passive document listeners; beforeinput deadline and deferred input check, logging only failed verdicts. Shared viewport snapshot stays while either composer probe remains. |
| Desktop render surface: `utils/threadRenderProbe.ts::reportThreadRenderProbe` | `webkit-desktop-blank-thread` is open. Base render tooling was explicitly retained permanently. | WebKit thread-open settle geometry/classification and log; no independent repeating timer in reporter. |
| Absent render target: `utils/threadRenderProbe.ts::reportAbsentThreadRender` | `notification-cold-open-loading` remains open; one fixed storage stall does not settle the other unexplained case. | One WebKit absent-target settle payload with admission and request age/count. |
| Device release log: `store/deviceDatabase.ts::releaseDeviceDatabases` | No recorded next-incident verdict identifying or excluding the old page as holder. | Transaction metadata enumeration and one log on release; actual database release is permanent. |
| Short follow landing: `components/chat/scrollState.ts::reportRideLandedShort` | `follow-ride-lands-short` remains open; ADR 0222 fixes structural holes but does not establish the remaining cause. | One boolean check per report attempt, at most one three-number log per ride. Correction remains independent. |

## Retained tooling and product measurements

| Candidate | Evidence and classification | Runtime cost |
|---|---|---|
| `utils/liveness.ts` | Registry explicitly resolved blackout investigation on 2026-06-30 and retained shared channel/startup/error tooling permanently. | Heartbeat storage, lifecycle/error listeners and startup logs; shared `postClientLog` requests. |
| Base `utils/threadRenderProbe.ts` | Same dated decision explicitly retains future-regression tooling; only extensions above are temporary. | Settle-time classification, geometry and logs on WebKit opens. |
| `utils/perfProbe.ts` | Registry explicitly retains general performance tooling permanently after closing click-lag. | Up to three supported PerformanceObservers; thresholded console output. |
| `hooks/useScrollMemory.ts` restore outcome/drift | Product restore instrumentation, with no temporary removal trigger. | One outcome log per content-anchor restore and settled drift geometry/log for line anchors. |
| `utils/swLivenessProbe.ts`, `swFetchPathProbe.ts`, `store/actions/send-probe.ts` | Health checks drive recovery, rather than collecting evidence alone. | Worker ping deadlines; paired health fetches; accepted-send 4 s confirmation timeout, conditional refetch/restart. |
| `store/restoreAdmissionWatchdog.ts` | Admission recovery is product behavior. | Admission deadline and conditional recovery/logging. |
| `utils/safeAreaFloor.ts`, `scrollbarGutter.ts`, SDK `tooltip.ts` | Hidden measurement elements supply layout values to product behavior. | Layout reads and resize observation; not diagnostic candidates. |
| `window_probe.rs` | Feature-gated standalone AppKit reproduction and regression tooling; header explicitly excludes shipped builds. | Zero in shipped clients; only runs when explicitly built and invoked. |

The producer and temporary-marker scan also covered native client diagnostics,
SDK sources, client-log callers, shell install sites and investigation references.
Native relaunch/crash diagnostics, IPC health and ordinary error breadcrumbs are
product recovery or permanent tooling. No additional unregistered temporary
probe was identified. Shared helper deletion must follow current callers:
request snapshots also feed render and crash diagnostics.

## Condition changes

The registry now requires recorded build/device/scenario/sample evidence.
Elapsed months and absence of log readers no longer substitute for verification.
The composer condition covers later causes instead of treating the autocorrect
finding as complete. Follow and notification investigations require exercised
landings/opens rather than empty logs. No investigation is closed by this pass.

Review should challenge inventory coverage, source-derived cost descriptions,
and whether any claimed closure has an actual audit. No new product defect was
established; this pass retains diagnostics for previously recorded investigations.
