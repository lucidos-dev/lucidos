# Frontend conventions: the reasons

`.claude/rules/frontend.md` lists the TypeScript and state conventions as short bullets. This page keeps the reasoning behind the long ones. Each section covers one rule area. ADRs hold the decisions, and this page holds what has no ADR.

## Filtering picker lists

`components/shared/useListFilter.ts` owns the query, printable-key capture, visibility, and Escape clearing for searchable lists. It extracts the behavior from `Dropdown.tsx`. The standard dropdown, `ModelSelectionPicker`, and `WidgetPickerItems` use it.

The field appears after typing on desktop and disappears when its query becomes empty. A coarse pointer or narrow layout always gets a field. Render it through the existing `SearchField` or `ControlFilter`, using their theme classes. Keep row navigation and focus placement in the host, since model steps and keyboard handoff have different focus targets.

`utils/listFilter.ts` matches every query term against the name and optional description without case sensitivity. Filter before computing keyboard indices. Register query clearing through the Escape stack; a host with nested steps can compose `clear` into its existing `useEscapeStep` handler.

## Drafts are threads

A compose draft lives in `threadMap` like any thread, with `meta.state === 'composing'`. `sendCompose` flips the state to `'active'` before it calls `sendMessage`. So `sendMessage` cannot tell a first send from a follow-up by thread state alone. Every plausible signal is wrong for one case: an optimistic insert, a focused draft, or the pre-flip. Lifecycle states are `'composing' | 'active' | 'discarded'`. Archive is orthogonal: an archived thread keeps `state: 'active'` and carries `section: 'archived'`.

### Per-draft selections

Draft content is per-draft (`composeDrafts[id]`). So are the dropdowns: target and scope, coding-agent backend, the Lucidos model and reasoning, the coding-agent model and reasoning. They live in `composeSelections[id]` (`store/composeSelections.ts`) as overrides, resolved by the `resolve*(threadId)` helpers.

- Model, agent and reasoning resolve as the draft's override, else the account default. `effectiveSendMode` has the same shape.
- Scope has no account preference. `resolveScope` falls back to `selectedScope` (localStorage, last used) only for the no-draft compose view. An existing draft resolves `override.scope ?? {kind:'lucidos'}`.
- Reading a mutable global such as `selectedScope` in a compose surface is a bug. One draft's change would leak to every other draft. That was the original regression.
- Coding-agent model and effort have no account default. `resolveCcModel` and `resolveCcReasoningEffort` return the override or `null`. A `null` makes the send omit the field, so the backend applies its default.

### Persistence

No compose interaction writes an account preference. The selection is stored in `thread_summaries.compose_selection`, beside the draft's text, images and mode. The debounced compose PUT (`pushNow` in `compose.ts`) writes it. The `ThreadComposeChanged` SSE event carries a `selection` field. A reload or a peer frame rehydrates it through `store/composeSync.ts` (ADR 0437). So a stored draft keeps its picks across a refresh and across devices.

- `chat_model`, `chat_reasoning_efforts` and `coding_agent_default` are account defaults and the live fallback of every override-less draft. A compose pick that wrote one would leak to every existing draft.
- A backend pick is the one deliberate exception. It writes the draft's `provider` override and the model row's preferred provider (`rememberProvider`, ADR 0247). The override keeps the draft on its backend. A draft that picked nothing follows the row.
- A scope pick writes `selectedScope`. It is the seed for the next new draft and no account preference. It is leak-safe because no existing draft reads it.

### The pending slot

A draft is created on the first keystroke. So the fresh compose view often has no focused draft while the user picks a destination, agent or model. Those picks go to `pendingComposeSelection`. A `null` or `undefined` thread id makes `getComposeSelectionOverride`, `patchComposeSelection` and `clearComposeSelection` use that slot. Every `resolve*` follows.

`ensureFocusedComposeThread` moves the pending picks onto the new draft. It also seeds `scope` from `selectedScope`: `seedComposeSelection({ scope: selectedScope.value, ...pending })`. The seed is required because a real draft no longer falls back to `selectedScope`. A pending scope pick wins over the seed. The first keystroke's PUT persists the seed.

### Surfaces

- Reads go through `resolve*`. A per-draft or pending write goes through `updateComposeSelection(threadId-or-null, …)`, which patches and schedules the PUT.
- A bare `setChatModelSelection` from a compose surface is the bug.
- Scope writes through `applyDestination`, which also updates `selectedScope`.
- The control menus tell compose context from an active thread. The coding-agent menu reads it as `!threadId`, since active threads pass the active-session id. The Lucidos menu takes a `composeContext` prop.
- At promotion, `sendCompose` binds the selections onto the send. Scope becomes `meta.repoId`, `codingAgentKind`, `codingAgentFolder` and `codingAgent`. Model, effort, cc-model and cc-effort go to `sendMessage` as `*Override` options. Then `clearComposeSelection(id)` runs.
- `sendMessage` uses an option when present. Otherwise a chat send reads the globals, and a coding-agent send carries no model or effort, because no draft carries an override.
- The active-thread menus are unchanged. The Lucidos picker writes the global `chat_model`. A coding-agent thread's pick is recorded on the engine, and locked while a turn runs (ADR 0542). Only the composing branch is per-draft. `CodingAgentControlMenu` takes `composeThreadId` for that.

## Loading and skeletons

### The delay gate has no exception

A timer never preempts running JavaScript. A response that lands inside `SPINNER_DELAY_MS` cancels a loader that never got a frame. A long synchronous render then paints nothing, and the transcript hit exactly that. The fix tried twice was a hold: raise the skeleton ahead of the gate, then apply the rows. It was reverted, and ADR 0081 records why.

A hold's two steps are the two reported symptoms. Raising the loader early is showing it instantly. Blocking the main thread right after is having no transition. Attack the render cost instead, by windowing, chunking or moving work off the main thread.

Components keep no `[loading, setLoading]` flag. A `Loadable` holds the state, and `loadable-guard.test.ts` enforces it. A `failed` branch renders `<LoadableError noun="items" error={loadable.error} />` (`components/shared/LoadableError.tsx`).

A loader the gate let through is a different matter: it stays opaque for `LOADER_MIN_VISIBLE_MS` (500 ms), so a load that ends just past the gate does not flash a skeleton (ADR 0555). The exit owners hold it, `<LoadingFade>` and `ThreadSkeletonOverlay`, through `useMinimumVisibleFlag`. While it holds, `<LoadingFade>` hides its content. The floor is legibility, so the speed slider leaves it alone. Do not add a floor to a caller or to `useDelayedFlag`, which also backs non-loader flags.

Full-screen surfaces gate the skeleton too, including the workspace picker. An earlier carve-out showed the skeleton at once when no competing content existed. It failed in practice. The picker renders its header and footer immediately, and its inline boot splash fades over it for about 0.65 s on every open. The sub-gate window is never a bare blank panel, so an ungated skeleton only blinked under the clearing splash on a fast local backend.

A list that fills from several reads is the one place a `.mini-spinner` joins a skeleton. Once some hits show, a trailing delay-gated spinner row names the reads still out, because a skeleton row there would read as one more hit. With no hits yet, the list draws its skeleton as usual. The worked example is `components/search/SearchEverywhere.tsx`.

### A loader the gate let through is opaque

Resting a loader transparent and ramping it up is a second gate stacked on the delay gate. On a wait of a few hundred ms the loader never reaches a legible opacity. The user sees the bare pane through it and reports a blank screen with no loader. One gate decides whether a loader is owed. Once it says yes, show it.

### Keep animated elements mounted

A remounted element cannot run a CSS transition, because the browser has no previous value to interpolate from. Preact matches unkeyed children by index. Two returns whose trees differ by a leading element rebuild everything under that index. `ThreadView` is the worked example, and it cost three rounds of reports.

Only a cold open crosses the two returns, so it reproduced on the iOS PWA and almost never on desktop. The cold-open return draws a bare wrap, and the main return leads with a header. The PWA is evicted constantly, so nearly every open is cold. Desktop users click between threads already in `threadMap`. Key the shared nodes in both returns. `components/chat/__tests__/thread-view-frame-identity.test.ts` pins it.

### Anchored popovers do not skeleton-swap

A skeleton cannot match the loaded body's height exactly. A popover with no covering chrome would resize on load. Render the body's structure at its natural height at once. Defer only the loaded values: dim and inert the value-bearing controls, and mark nothing active until the data lands. Render static explanatory text right away so it pins the height. Only opacity and active state then change.

The canonical example is the gateway picker's Network access popover (`components/picker/NetworkAccessPopover.tsx`). Its body is a pure function of `Loadable<T>`, so the load and settle states are unit-testable. Its own bugs produced three rules:

- A form's in-progress edit lives inside the loaded state. One `Loadable<{config, draft}>` makes a draft that outlived its config unrepresentable. The popover once held the config in one signal and three draft fields in three more. None reset on close. So cancelling an edit and reopening showed the abandoned click as the active value until the refetch corrected it. Reopening resets to `loading`, which carries no draft.
- Reopening refetches. The popover shows what is stored, not what was last clicked. A cached-config fast path makes a stale draft look settled.
- A row whose presence depends on the loaded value stays mounted and animates its height (`grid-template-rows: 0fr` to `1fr`, `overflow: hidden` on the inner element). Conditional mounting turns "the saved value needs an extra field" into a jump at settle time. Keep the row's leading gap as padding inside the clipped box. A margin survives the clip and leaves a phantom gap when the row is shut.

### Self-skeletonizing rows

Every `Loadable`-backed list or tree surface ships a skeleton built from the row's own component. A hand-drawn generic shimmer or a separate skeleton tree drifts the moment the row changes. Built from the row, the leaves render real content when loaded and `.sk-bar` shimmer inside the provider. The loaded path is unchanged, so the two cannot fall out of sync.

- Wire the row with optional props and a `useSkeleton()` check. Route text through `<SkText>`. Route buttons, chips, icons and dots through `<SkBlock>` (both in `components/shared/Skeleton.tsx`). Gate handlers and optional structure on `!sk` or `(sk || cond)`.
- Render N copies through `<ListSkeletonOf row={() => <MyRow/>} fill? containerClass=… />`. Pass the real list's container class so spacing mirrors. Pass `fill` for a full-pane list. It measures the pane and the real row height.
- Trees vary rows by index through the `row(i)` thunk (see `folderTreeSkeletonRow`).
- A surface that cannot take the `<div>` wrapper still self-skeletonizes the real row markup inside a `<SkeletonProvider>`. That covers a `<ul>/<li>` list and brand-skinned surfaces such as the picker and the control-panel switcher.
- Shapes that are not rows reuse the idea. A code block draws its own box with `SkText` lines (`StepCodeBlock`). A source file draws `LineNumberedCode`'s gutter (`FileSourceSkeleton`). A diff draws `DiffView` cards (`DiffSkeleton`). A document takes `ProseSkeleton`. A form draws its `.form-group` boxes with real labels (`FormSkeleton`).
- `.sk-bar` is translucent, a tint of the text colour with an accent shine, so it shows on any background. A brand-coloured surface overrides only `--sk-base` and `--sk-shine`. `styles/__tests__/sk-bar-shows-on-any-surface.test.ts` lets no other rule repaint a bar.
- A code-split view draws nothing, not even its skeleton, until its chunk lands. Every rebuild renames the chunks. So what must appear at once is eager (`SettingsHome`, the notification views). `ContentPane` prefetches the rest when idle (`prefetchWhenIdle`, ADR 0288).
- A list that fills from several reads shows a trailing delay-gated `.mini-spinner` row once some hits show. A skeleton row there would read as one more hit. See `components/search/SearchEverywhere.tsx`.

### A single value, a single control

A single value inside otherwise-real markup gets no skeleton. Hold the surrounding control's box and leave the slot empty until the value lands. Such a value is read once at app startup, so its missing window is a cold load. An iOS PWA is evicted constantly, so an ungated bar would shimmer on almost every launch. Right-anchor the box so nothing already drawn moves when the value arrives, as the pill in `workspacesMenuRow` does with its chevron.

A single control fed by its own read gets a skeleton that wears the real control's box. A settings page often has several controls on separate requests, each rendered on its own flag. The page then steps down as they arrive. 

Reserve the space with `<DropdownSkeleton w=…/>` from `components/shared/Dropdown.tsx`. It renders the trigger's own `.dropdown-trigger` box and real chevron, so padding, border, radius, gap and font metrics come from one rule. Only the label width is a guess. A hand-sized `<SkBlock>` drifts the next time someone re-pads the trigger. A skeleton without the chevron is narrower than the control by one chevron, which is the shift it was added to prevent.

Two things bite inside `<LoadingFade>`. The fade wrapper becomes the flex or grid item in the parent row. So a `flex-shrink: 0` meant for the control has to reach the wrapper (`<LoadingFade class=…>`, `.dropdown-slot`). And a slot that renders nothing once settled must unmount with it, or the row's `gap` opens around an empty child. The worked example is `DropdownSlot` in `components/settings/BackupSection.tsx`.

### One delay gate per surface

Reserving each slot fixes the layout shift but not the timing. With one `useDelayedFlag` per slot, every gate arms when its own read starts, so the surface shimmers in waves. Arm one gate on the first render and hold it until the last first-load read settles. AND it with each slot's own pending flag. Every owed skeleton then appears together. Each still clears on its own read, so nothing loaded is withheld.

- A slot's pending flag starts at the first render, not when its own fetch starts. Some reads are chained: the Backup health card's status is per destination, so it waits on the provider registry. Gating that read on its own `Loadable` leaves a zero-height box until the chain reaches it. Then a box grows at the top of a surface that already looks settled.
- Derive that flag synchronously, never from the effect that starts the read. An effect runs after the render that unblocked it. A flag reading `status === 'loading'` goes false for one frame, and `useDelayedFlag` resets on any drop. The skeleton blinks out and returns 300 ms later as a second wave. Key it on what the render knows (Backup uses `selectedReady`).
- Reads that land in one row settle together. Fetch them in one `Promise.allSettled` and apply the results in one batch.

`backupSlotsPending` in `components/settings/BackupSection.tsx` is the decision function. `__tests__/backup-skeleton-timing.test.ts` pins it.

## Expand, collapse and exit

An inline toggle carries `aria-expanded`, and its content rolls through `<Disclosure>`. A popover toggle pairs `aria-expanded` with `aria-haspopup` and opens through `<Overlay>`.

- A fold is one element, never two swapped. `SideQuestionCard.tsx` is the worked case. Its head is the toggle and its body sits in a `<Disclosure>`. Replacing the card with a separate row element cannot animate.
- A mount gated on an open flag rolls too, wherever its toggle lives. `{searchOpen.value && <Bar />}` snaps even when the toggle is a header button in another file. The Apps and Plugins search bar (`components/apps/AppSearchBar.tsx`) is the worked case.
- `<Disclosure open>` (`components/shared/Disclosure.tsx`) gives the disclosure roll from one curve and length in `utils/disclosureMotion.ts`. Pass the children unconditionally. The component unmounts them when closed and keeps them, inert, for the exit. A navigation that opens it to land on a row inside passes `instant`, since a scroll mid-roll measures a moving box. A suite that flattens vnodes with a bare call mocks it with `components/shared/__tests__/disclosureStub.ts`.
- `<ArrivalList>` (`components/shared/ArrivalList.tsx`) rolls in a row that arrives in a live list. Call `useArrivals` from the view that stays mounted, not from a collapsible section. The Changes view is the worked case.

A chevron turns with the roll, on a `transition` of its `transform`.

Whatever animates in animates out, unasked. An instant unmount at dismiss is a bug.

- Prefer a drawing held for the exit: the element, or a non-interactive drawing of it, held with `useLingeringFlag` and a scaled timer. The overlay is shut by then, so the UI behind is live again at once.
- An overlay that stays open through its exit returns `false` from `onClose` until it has gone, as `closeDrawer` does. Otherwise it keeps the UI behind inert and swallows the next tap.
- The menu drawer (`components/layout/Drawer.tsx`) slides out before it unmounts. The thread drawer's list (`ThreadDrawer.tsx`) stays mounted through its pane's collapse with `useLingeringFlag`.
- Every backdrop modal already fades out. `<Overlay>` leaves an inert copy of its last frame (`components/shared/overlayExitDrawing.ts`), which covers a caller that unmounts the whole `<Overlay>`. A panel past `MAX_DRAWN_ELEMENTS` closes at once, since copying it would stall the close.

## Errors

- `throwIfNotOk` (`api/client/_core.ts`) is the one place a response body becomes a `reason`. A JSON `{error}` or `{reason}` field is kept as written. Markup becomes a short status phrase. Plain text keeps its first line, whitespace-collapsed and clamped.
- `showToast` is the second net. It clamps every title and message (`clampToastText`) and flattens an `error` to one line. The gateway's 503 boot splash is an 8 KB HTML page. It once rendered as a list of its own `<meta>` tags, and a web page became a card covering the transcript.
- A toast's title is explicit: `showToast(message, type, { title })`. Only the title is bold. A newline in a message is a line break.
- An error message names the entity and its origin. Say what failed (`App "demo-director" no longer exists`, not `App no longer exists`). When the action was not a direct click, say where it came from (`(requested by thread "X")`). A `NavigationRequested` from a sibling thread once toasted "App no longer exists" with no id and no source, for an app that existed on disk.
- A cached projection never proves that something does not exist. Lists such as `appsList` and `artifacts` are caches that SSE events refresh, and they go stale when a sibling thread mutates state. A "gone" verdict must follow a re-fetch of the source of truth (`openAppById` re-scans disk on a miss). A `list.some(...)` pre-check that short-circuits the re-fetch reports live entities as deleted.
- A list whose freshness depends on a refresh event needs every mutation path to emit it. `loadApps()` re-scans `data/apps/`, and the cache updates live only because something emits the `App*` or `Artifact*` event that the `entityReferences` arm reloads on. The chat `write_file` tool once emitted `App*` only for `artifacts/` paths. Apps created by raw file writes never refreshed the list.

### Best-effort telemetry

A narrow `console.warn` without a toast or `Loadable` failure is acceptable for work that runs without user intent and recovers on its own. Examples are presence pongs, device-visibility heartbeats and push-subscription keepalive. Others are startup probes the user did not start and tab-close `keepalive` flushes where no toast can render. Background breadcrumbs also qualify when the user-facing error surface lives elsewhere. A parallel `postAppCapture` delivers the failure to the LLM.

Every carve-out site needs three things:

1. A comment above the `console.warn` saying why a toast is wrong and how the user still finds out if it matters. No comment means no carve-out.
2. No user intent on the failed line. A user who clicked a button is owed a toast, even for cancel or dismiss.
3. Self-recovery. The next probe, heartbeat or push attempt must succeed silently or escalate through a path that surfaces to the user.

A scheduled path that could fail repeatedly (polling) prefers the tracked failure counter, `utils/failureCounter.ts`. It stays silent below a threshold, toasts once at threshold N, and resets on success.

## A UI surface owns its subscription

ADR 0118 holds the full reasoning.

- Reading a store signal is the subscription. A surface that fetches its own data subscribes through a version counter the dispatcher bumps: `useVersionedRefresh(xVersion.value, paused, reload)`. Precedents are `backupPreferencesVersion`, `mcpServersVersion`, `webhooksVersion` and `permissionGrantsVersion`.
- The stream never replays a dropped frame. After a gap (a reopen after an error, or `Lagged`), the one gap reconcile bumps every counter and clears every progress signal. A version counter is `gapRefreshedVersion()` and an ephemeral progress signal is `gapClearedProgress<T>()`, both in `store/eventGap.ts`. Never hand-write a repair into either path. `an-event-gap-repairs-every-sse-fed-signal.test.ts` fails a `*Version` or `*Progress` export built without a factory. A signal no frame feeds takes a `NOT_GAP_STATE` row saying why.
- A poll, an interval, a focus or visibility listener, or an invalidate-on-navigation each makes one symptom go away and leaves the rule unenforced. `store/actions/sse-event-coverage.test.ts` scans the subscriber modules for all four.
- `useServerBackedField` (`hooks/useServerBackedField.ts`) returns the served value while untouched and holds no copy, so a frame repaints it. Touched, it keeps the user's draft. Seeding a `useState` from an entity runs the initializer once per mount. A component keyed on an entity id then keeps its first snapshot. That left the trigger detail page showing an old group after three `update_trigger` calls landed.
- A whole-editor draft pauses the re-read instead (`AllowlistEditor`). The frame lands when the draft is saved or reverted.
- `sse-event-coverage.test.ts` reads `RESERVED_TYPE_NAMES` out of the Rust source. Each `SystemEvent` name needs an arm in `thread-sync.ts` or `entityReferences.ts`, or a row in that test's `NO_UI_STATE` table.

## Navigation that lands content

Skipping `revealContentPane()` is a silent no-op on whichever surface was not already visible. The user's tap did nothing. On desktop it lands the view but leaves Tab stuck on the previously focused pane.

- The desktop focus half is signal-only, matching the pointer-down `focusPane`. It sets `focusedPane` and does not move DOM focus. The first Tab pulls focus in through `paneTabTarget`'s pull-in branch.
- One exception keeps the marker put: an agent navigation while the reader types. `routeThreadNavigation` calls `holdFocusedPaneWhileTyping` first. With DOM focus in a text field in another pane, the next reveal leaves `focusedPane` alone. Moving the marker alone would send the next Tab away from where the reader types. The reader's own navigation takes no hold.
- `revealThreadPane()` (`store/actions/pane.ts`) is the mirror for a landing on a thread: an existing one, the empty compose view, or a new one spawned from another pane. It re-expands a collapsed split (`splitRatio <= 0`) and re-activates the Threads pane group. Mobile swipes to the thread pane.
- Its focus half normally runs only from the cross-group case (`focusedPane === 'content'`), so drawer arrow browsing is not disturbed. A collapse it just undid takes the focus unconditionally. The pane was invisible, so a marker pointing into it is stale.
- Both halves are required, and the re-expand gets forgotten. With the thread pane collapsed, a thread link clicked from the content pane moved `focusedThreadId` behind a zero-width pane. Nothing on screen changed and the click read as a no-op. Mobile never saw it, because panes there are navigated rather than collapsed.
- Callers of `revealThreadPane`: `focusThread`, `unfocusThread` (the compose view), the raw-new path of `sendMessage`, and `sendSeededPrompt` (`store/actions/compose.ts`). `sendSeededPrompt` is behind the setup interview, a release-notice action, and Discuss on a notification or webhook bar. It reveals before the send, because `ensureFocusedComposeThread` allocates the id client-side.
- `{ revealPane: false }` on `focusThread` and `unfocusThread` means bookkeeping, not navigation. `ThreadView` clears a stale `focusedThreadId` during render, and on mobile every pane is mounted at once. A reveal there would swipe a user on the content pane to the thread pane mid-render. The post-archive hand-off (`handleArchiveThread`) passes it too, so a user archiving row after row is not swiped out of the drawer. A user-intent focus always reveals.
- User-intent helpers call `revealContentPane()` at the end: `switchMenuItem`, `openSettingsSubview`, `landOnAccountsWithOverlay`, `openApp`, `openAppById`, `openFilePreview`, `openUrl`, `openRepoFilePreview`, `openEncodedRepoFilePreview`, `navigateToTrigger`, and every `handleNavigationRequest` branch that lands content.
- `setActiveMenu` (`store/actions/menu.ts`) is plumbing and does not swipe panes. It once carried a conditional `navigateToPane('content')` gated on `item !== prev && mobileView === 'thread'`. The gate dropped the swipe when the user re-tapped the current item. Callers are explicit now.
- A bare `navigateToPane('content'|'thread')` under an `isMobile()` gate covers only the mobile half. A bare `navigateToPane('thread')` after `unfocusThread` or `focusThread` is the smell that the reveal belongs in the helper.

The panel nav history keeps one invariant: the row at the cursor describes what is on screen. A navigation earns a row by calling `pushNavState()` in the same task as its state change. Any other write to `activeMenuItem`, `settingsSubview`, `panelOverlay` or `wipPreviewThreadId` folds into the current row on the next microtask (`foldUnpushedChange` in `store/actions/navigation.ts`). A close path needs no bookkeeping. A push after an `await` is the one thing to avoid, because the fold runs first and overwrites the row you came from.

The navigation cover (`docs/glossary.md`) smooths the view swap on every change of the content view key. A new entry point adds no fade of its own. A new view owes identity.

A `PanelOverlay` variant whose payload picks one of several things resolves in `components/layout/contentViewKey.ts`. A variant that returns its bare type while showing several things breaks scroll memory.

## Pane resize and headers

The clamped divider contract is in `docs/glossary.md` § Clamped divider and ADR 0056.

- A drag never collapses a pane. The collapse-state attributes (`data-thread-collapsed`, `data-content-collapsed`, `data-thread-drawer-open`) flip at a ratio of exactly 0 or 1. A clamped drag cannot reach either, so a mid-drag flip is unreachable. Those flips swap header icon groups between hosts while the pointer wiggles across a pane edge. That was the "icons dance between the headers" bug. The earlier deferred snap (a free drag corrected about 400 ms after release) existed because a drag could collapse.
- Pane minimums are derived from the root font size and live in `store/paneMinimums.ts`. Everything they size is rem-authored, and a px constant is right at one UI scale only. Reading one is a DOM read, so `splitHelpers.ts` stays pure: the caller measures (`splitBounds()`) and the helpers compute. The three minimums are weighed against each other. From 112.5% ui-scale on a 1280 px screen they stop summing, and the Canvas yields.
- None of the three varies by client. The drawer's floor pays the traffic-lights lead in the browser too (ADR 0058). `data-titlebar-overlay` decides how the header row is laid out and never how narrow a pane may get.
- A pane that cannot fit its floor yields and never squeezes (ADRs 0495 and 0498). With room for one split pane, an open Canvas is drawn collapsed. Beside the Conversation's floor, the rail yields before the drawer. The Conversation never yields. The stored `splitRatio`, `threadDrawerOpen`, `threadDrawerWidth`, `dashboardRailShown` and `dashboardRailWidth` stay put.
- Read `drawnSplitRatio` wherever you describe the screen and write `splitRatio`. Show the Canvas through `contentShownRatio`, which swaps the panes when only one fits. A bare `DEFAULT_SPLIT_RATIO` there yields again, and the click does nothing.
- Read the drawn side panes as `drawnDrawerShown`, `drawnDrawerWidth`, `drawnRailShown` and `drawnRailWidth`. A side pane's toggle reads what is drawn, and a tap swaps, else is disabled. Showing a yielded rail closes the drawer for real. Showing the drawer needs no swap. A pane that cannot fit alone (`threadListCanShow`, `dashboardRailCanShow`) gets an `aria-disabled` toggle with a "too narrow" tooltip, and a shortcut toasts the same reason. A drag pins every yield (`beginPaneResize`), and `endPaneResize` decides them.
- A header section with a `left` or `width` transition goes into the `:root[data-pane-resizing]` `transition: none` block in `styles/panels/shell.css`. Otherwise it lags the panes during a drag. Header sections and panes share `var(--duration-slow) ease` for geometry transitions so snaps arrive together.
- Header regions hide by fading: `opacity: 0; visibility: hidden; pointer-events: none` on the same transitions. `display: none`, a removed conditional render, or a keyframe keyed on the state attribute pops the element out at the start of the pane animation. Pane content stays mounted through the exit (`useLingeringFlag` in `hooks/useDelayedLoading.ts`).

### A single control is never two copies

The bullet above covers a whole region leaving with its pane. A lone control is the opposite case, and both obvious shapes are reported bugs. Two mounted copies crossfading show two half-transparent icons converging on one x. One copy fading in place leaves a dimmed ghost where another control slides in. Four rules follow:

- First ask whether it needs a second position at all. The drawer toggle once travelled with the drawer, and a tester could not find it (ADR 0276). It now rests in the header's corner in both drawer states. A neighbour that slides toward it clips at its edge (`--threads-row-lead`).
- A control that must move is one element whose `left` transitions, with no `opacity`. `left` interpolates between two resolved lengths, so every position serves the animation in both directions. A negative `left` once sent the toggle out correctly and brought it back inside the drawer row.
- A control that must disappear shrinks in place. Its width runs to 0 behind a constant `clip-path` that leaves room for the focus ring. Never `overflow: clip`, which does not transition: it lifts on the first frame back and flashes the whole control.
- Unless its box stays reserved. A shrink slides its neighbours, and for a control that comes and goes with a mode, that slide is the motion the user notices. When nothing moves into the space, no ghost is possible. The control then fades in place in a slot that keeps its box. The threads Filter button under the Ongoing grouping is the case (`.filter-slot`, `styles/__tests__/filter-fades.test.ts`).

The worked example is `.thread-toggle-slot` in `styles/panels/shell.css`. `styles/__tests__/header-drawer-toggle-pinned.test.ts` and `e2e/header-drawer-toggle-pinned-desktop.spec.ts` pin it. A geometry transition also puts a control under the kill-list bullet, so the two land in the same change.

### Header titles centre on the pane

All four title rows have one shape. They are the desktop drawer's `.threads-header-title`, the Conversation pane's `.pane-header-brand-label`, the Canvas pane's `.pane-header-content-title`, and `.header-nav-cluster` on both mobile rows. The leading and trailing clusters stay in flow at the row's ends. The title cluster is absolutely centred over them at `clamp(min-span, 100% - 2 * <reserve>, span)`. On the Canvas row the `span` is itself a clamp. It grows past `--desktop-nav-span` only out of room beyond `--content-unfolded-reserve`, up to `--desktop-content-title-span-max` (ADR 0399).

- Centre on the pane, not on the gap between clusters. A `flex: 1` middle moves the title whenever either cluster changes width. The Canvas row was the last built that way. Its trailing cluster is 1 to 3 icon boxes wide, depending on the content view. The title and both chevrons slid as the ⋯ trigger came and went.
- Derive the reserve from the widest cluster the row can hold, written as icon boxes and gaps against `--header-icon-box`. Source scans check the copies (`styles/__tests__/content-header-title-centring.test.ts`, `header-band-centering.test.ts`). The clamp is the non-overlap guarantee. An earlier `translate(-50%)` title with `max-width: calc(100% - 12rem)` let the title slide under the icons. Below the clamp's min-span arm, `useHeaderActionCollapse` folds the actions into ⋯.

### Keyboard resize and shortcuts

`stepThreadPaneWidth` and `stepThreadDrawerWidth` (`store/actions/pane.ts`) move a divider by `KEYBOARD_RESIZE_STEP_PX` per press. Pure math lives in `computeStepRatio` and `computeDrawerStepWidth`. They clamp to the same minimums as a drag and never collapse. The keyboard once clamped first and the drag caught up (ADR 0056). A new resize entry point takes the clamp from the same helpers. One difference remains: a keystroke against a collapsed pane is a no-op, and a drag on its divider re-expands it to the minimum.

Two desktop shortcuts move between Conversation and Canvas (`docs/glossary.md` §§ Pane group, Split). `⌘⇧↵` (`toggleMaximizeFocusedPaneGroup`) toggles the focused pane group full-width through `setSplitRatio(1|0)` and restores the remembered ratio on a second press. `⌘⇧1` (`focusOrToggleThreadDrawer`) opens and focuses, focuses, then closes. It moves DOM focus into the drawer so its list navigation works, and it re-expands the Conversation side first if collapsed. Both are desktop-only (`isMobile()`) and rebindable. The drawer icon stays a pure show/hide (`toggleThreads`).

### Shortcuts from inside an iframe

A keydown inside an `<iframe>` never bubbles to the host `document`. The global handler (`useKeyboardShortcuts.ts`) never matches it and never calls `preventDefault()`. Focus a file or HTML preview and press `⌘⇧↵`, and Chrome opened its page context menu instead of maximizing.

- App iframes forward their chords over postMessage (`packages/lucidos-sdk/src/keyboardForward.ts`, `dispatchForwardedChord`). The host pushes its bindings to each frame on the `keybindings` channel (`store/actions/app-keybindings.ts`), so the SDK cancels the browser default for a host-bound chord synchronously.
- The HTML artifact preview runs at an opaque origin (ADR 0322). A script stamped into its srcdoc forwards bound chords and Escape to `dispatchForwardedChord` (`components/files/previewFrameBridge.ts`). The host stamps the current bindings into that script, and the frame matches them synchronously. The host accepts a message only from that frame's own window, at origin `"null"`, with the render's nonce. Link clicks, sibling downloads, fragment scrolls, pane swipes and the find bar's requests ride the same bridge. The bridge removes its own script element so the artifact cannot read the nonce, and posts only for a trusted event.
- The PDF preview is same-origin and runs no SDK. `bridgePreviewIframeShortcuts` (`components/files/previewIframeShortcuts.ts`, wired through the iframe `onLoad`) listens for `keydown` on its own `contentDocument` in the capture phase. `dispatchPreviewIframeShortcut` matches the same registry, calls `preventDefault()` for a host chord, and reconciles `focusedPane = 'content'` first. A cross-origin frame throws on `contentDocument` and no-ops.

## Overlays

Two contracts, both required: an outside click closes an overlay, and it activates nothing else. A CSS-only backdrop close dismisses but leaves the underlying click free to fire another button. A bare `pointerdown` listener that calls `onClose` without swallowing the paired event does the same. A user dismissing a settings popover could then send a chat message, open an app, start a thread or trigger a destructive action. While an overlay is open the UI behind is inert, which is the hover analog of the swallow. A backdrop-less popover makes the UI behind inert too.

`<Overlay>` (`components/shared/Overlay.tsx`) bakes the contract in, so an individual overlay cannot drop it. The SearchEverywhere "second tap reopens" bug came from passing `anchor={null}`. `useDismissOnOutside` and `overlayStack` are its internals and have no other callers. The older `ModalOverlay` was deleted because it dismissed without swallowing and could not serve click-through overlays.

### Anchors

The anchor is exempt from the outside-pointerdown dismiss, so re-activating the toggle closes through the toggle's own handler. With `anchor={null}` on touch, the outside `pointerdown` closes the overlay and the toggle's `touchend` re-flips it open. It never closes. When toggle and overlay live in different components, stash the element in a signal at click time. `controlPanelAnchor`, `drawerAnchor` and `searchEverywhereAnchor` are the precedents.

An overlay opened by a gesture has no toggle, and `anchor={null}` is right there. The mobile drawer row draws no ⋯. A long press opens its menu (`useRowActionsGesture`, `<OverflowMenu hostOpener>`). A short tap never opens it, so nothing re-flips it. While it is open, `:root[data-overlay-open]` inerts the shell, so no second hold can arm. 

A desktop row's right-click opens the same menu. The ⋯ is still drawn there, so it stays the anchor. The row is never the anchor: an anchor is exempt from the dismiss, and the row's click focuses a thread, so one tap would dismiss the menu and navigate. Positioning still tracks the row, because `useAnchoredPosition` takes its element separately.

A gesture-opened overlay pays for the tail of its gesture. The hold's lift dispatches a click whose `pointerdown` fired before the listeners existed. Item 4's fallback read that click as synthetic and dismissed the menu on release, every time. `useDismissOnOutside` asks `primaryPointerIsDown()` (`utils/pointerPress.ts`) as it installs. An overlay that opened under a press spends that one click. Trusted presses only: a dispatched `PointerEvent` pairs with no click, so counting it would eat the next synthetic dismiss and break the canary spec.

### The numbered contract

Mechanism: `makeDismissHandlers` and `store/overlayStack.ts`.

0. Only the topmost panel answers a pointer. Every open overlay installs its own `document` listener, and each asks whether the target is outside its own panel. A sibling's panel read as outside, so the lower overlay dismissed itself invisibly behind the upper one. `<Overlay>` passes `() => topPanelOverlay()?.id === idRef.current` as `isTop`. It gates the pointerdown dismiss, the swallow-arm, the synthetic-click fallback and the fallback Escape.

   Never use the raw `topOverlay`: an Escape-only registrant draws nothing, and `ModelSelectionPicker`'s sits above its own host, which killed dismiss on every model menu while a tier row was open. `<Overlay>` registers in a `useLayoutEffect` beside those listeners, because split across phases its first frame answered nothing.
   - A mouse gesture is two events in two tasks. The upper overlay consumes the `pointerdown` and unmounts between them. The lower one is then the top panel when the paired `click` lands, read it as unpaired, and closed anyway. So a handler records an outside primary `pointerdown` it declined, and the fallback skips the click pairing with it. Primary only, cleared on `pointercancel` and `touchcancel`, because a pairing that outlives its gesture eats the next synthetic click.
   - The top panel must also be on top in paint order. Every `.modal-overlay` shares `--z-modal`, so paint order fell back to `OverlayLayer`'s child list, where `<ConfirmAndPromptSlot />` sits ahead of most slots. A confirm raised by a modal was drawn behind it. It still took the Escape and swallowed the click, and the shell stayed inert, so the reader saw a dead button.

     `<Overlay>` now sets `z-index: calc(var(--z-modal) + overlayStackDepth(...))` on its backdrop container. `MAX_MODAL_STACK_DEPTH` caps the band under `.ui-blocking-overlay`. Backdrop mode only. `store/__tests__/overlay-stack-depth.test.ts` and `components/shared/__tests__/a-confirm-opens-above-the-modal-that-asked.test.tsx` pin it.
   - An open modal also outranks the toast layer, unless a toast cannot wait. A standing offer says the same thing after the modal closes. An error or warning must be seen while the modal is up. `toastStackUrgency` (`components/shared/toastUrgency.ts`) answers per stack, since `.toast-container` carries the z-index. The rule stands down under `[data-ui-blocked]`.
1. A `pointerdown` outside the panel and anchor calls `onClose()` and arms "swallow the next paired event". The swallow lives on a one-shot `document` listener that outlives the overlay's unmount (`installPairedSwallow`). Closing tears the overlay's own listeners down before the paired `touchend` or `click`. The one-shot disarms on the first swallowed event, on `touchcancel` or `pointercancel`, and on the next primary `pointerdown`. That last bound matters: the arm belongs to one gesture, and a stranded arm eats the user's next tap. See `docs/plans/2026-08-28-a-swallowed-tap-says-so.md`.
2. The paired `touchend` outside the panel and anchor is captured at `document` in the capture phase. It precedes the target button's own bubble-phase `onTouchEnd`. It gets `stopPropagation` and `preventDefault`. Touch buttons that run their action on `onTouchEnd` and cancel the synthetic click would otherwise fire on the same tap that dismissed the overlay.

   The iOS keyboard-nudge pattern in `composeHandlers` is one. That was the compose-on-first-tap bug. Anchor and inside-panel touches are never swallowed.

   The overlay's own open-gated `touchend` handler also swallows, for the same-task case. The two are complementary.
3. The paired `click` (the mouse case) is captured at `document` and stopped. On touch the `touchend` already consumed the one-shot.
4. A `click` with no preceding outside pointerdown takes a click-capture fallback. Such a click is a synthetic `HTMLElement.click()` from a keyboard shortcut, an e2e driver or another program. The fallback runs the same `isOutsidePointerTarget` check, then `onClose()` and the swallow. Without it the contract breaks for every caller that dismisses through synthetic clicks. `e2e/overlay-dismiss-swallow-all.spec.ts` is the canary, over the drawer row's overflow menu.
   - A click dispatched within an inside click's dispatch is inside too, for that dispatch only. Capture opens the window on the inside click. A document-level bubble listener closes it when the same event comes back up. A task-scoped backstop covers a handler that stops propagation. A boolean on a timer would exempt every click in the rest of the task.
   - Without the carve-out, an overlay whose action is `someInput.click()` outside its panel reads its own action as an outside click and calls `preventDefault()`. On a file input, the cancelled default action is the file chooser. So the item does nothing and logs nothing. The composer attach menu's File item was dead that way for three months.
   - Deferring the `.click()` out of the gesture is no fix. It drops transient user activation, which makes a file picker unreliable on iOS.
5. The anchor is exempt, so re-activating it toggles through the caller's `onClick` or `onTouchEnd`. So is the toast layer (`.toast-container`), since a toast stays live over an overlay. A backdrop-less popover light-dismisses on it, as `popover="auto"` does: it closes and the same tap presses the toast button. A backdrop modal stays open, so the X on an error toast never discards a half-filled form.
6. Escape dismisses through the central LIFO `overlayStack`. One capture-phase dispatcher (`useKeyboardShortcuts`) pops the top overlay, so stacked overlays close newest-first and per-instance listeners never race. It calls `stopPropagation`, which also shadows the hook's own Escape so `onClose` fires once.

While an element is natively fullscreen the dispatcher stands down (`dispatchEscape` returns `'fullscreen'`). The browser takes that Escape to exit fullscreen, and nothing in a keydown handler can stop it. Dismissing as well would make one keypress do two things.

It still calls `stopPropagation` without `preventDefault`, so the hook's bubble-phase Escape cannot close the overlay behind the stand-down's back. Pseudo-fullscreen registers on the same stack before the overlay, so LIFO closes the overlay first.
7. Inert-behind: while any `<Overlay>` is open, `<html>` gets `data-overlay-open`, ref-counted across stacked overlays. CSS sets `.app-shell > * { pointer-events: none }` and re-enables three things with `pointer-events: auto`:
   - The overlay panels (`[data-overlay-panel]`).
   - The toggle that opened the overlay (`[data-overlay-anchor]`, set by `<Overlay>` on its `anchor`). The anchor lives inside `.app-shell`. Without this, the toggle cannot fire its own handler and rule 5's exemption is dead. A careful pointer or Playwright cannot land on the inert toggle at all.
   - The overlay layer's mount (`[data-overlay-layer]`). While an app is natively fullscreen, `OverlayLayer` moves the group into a mount inside the fullscreen app panel, the only subtree the browser paints. That mount sits deep inside `.app-shell` and would inherit the inert.

   The inert targets `.app-shell`'s children, never `.app-shell` itself. `.app-shell` must stay a real hit target so an outside click lands on it and dismisses. A backdrop-less popover has no scrim. A click on an inert `.app-shell` would fall through to `#app`, where the outside-click path finds no target. That regression broke `message-route-panel` and `cc-slash-menu` clicks.

   `Toast` and the modal scrims render outside `.app-shell`, so they stay live. `pointer-events` is used rather than `inert` because a descendant can override it. In e2e, `force`-tap an inert non-anchor target. A real finger lands on `.app-shell` the same way.
8. Keyboard focus (ADR 0335, `components/shared/overlayFocus.ts`): the document Tab branch asks `handleOverlayTab` before the pane trap, walking the open overlays top first. A dialog (a backdrop, or `ariaModal`) contains Tab. A Tab from outside moves focus in, Tab wraps at the panel's ends, and focus on an unlisted element steps to its listed neighbour (`tabTargetWithin`). An anchored popover wraps Tab only while focus is inside it, and a Tab from outside closes it and moves on. `<Overlay>` focuses into a dialog on open and hands focus back to the opener on close, keyboard devices only. `components/shared/__tests__/overlay-tab.test.ts` pins it.

### `onClose` and the stack

`onClose` may return `false` to declare a no-op. One case is an overlay already on its way out through a close animation while the originating signal is still `true`. Both the pointerdown path and the click-capture fallback honour it. A `false` leaves the suppressor disarmed and skips the inline swallow, so the user's tap on a sibling button reaches its handler. `closeDrawer` returns `false` during its 200 ms slide-out so the hook stops eating neighbour taps mid-animation.

`<Overlay>` is the only thing that registers a panel into `overlayStack`. The stack also takes Escape-only registrants, which `hasPanel` marks out for `topPanelOverlay`: `pseudo-fullscreen`, the thread filter, the find bar, and a step inside a panel through `useEscapeStep`. Unit tests cover `makeDismissHandlers` (`hooks/useAnchoredPopover.test.ts`), the stack's two questions (`store/__tests__/overlay-stack-top-panel.test.ts`) and the `<Overlay>` tripwires (`components/shared/__tests__/overlay-contract.test.ts`). End-to-end coverage is `e2e/search-everywhere-close-mobile.spec.ts` (re-tapping the anchor closes) and `e2e/overlay-compose-dismiss-mobile.spec.ts` (a touch tap on a sibling `touchend` button dismisses without firing the action).

The desktop dashboard's rail (`components/dashboard/DashboardRail.tsx`) is no overlay. It sits inside `.app-shell` and takes room beside the header and panes, so an open overlay inerts it with the rest of the shell.

A step inside a panel answers Escape through the stack too. A keydown handler cannot, because the dispatcher runs in the capture phase and stops propagation. `useEscapeStep` (`hooks/useEscapeStep.ts`) pushes a registrant while there is somewhere to step back to. Registering later than the panel puts the step on top of it. Its callers are `ModelSelectionPicker`'s tier and provider steps and the waiting panel's drill-in.
