# 0127: Every panel-nav entry restores its overlay; there are no transient navigations

- **Status**: Accepted
- **Date**: 2026-08-25

## Context

`restoreState` in `crates/lucidos-app/src/store/actions/navigation.ts` used to run
a second guard beside `suppressUrlPreview`, called `isTransientForm`. It nulled
the overlay of four form families: a pending `plugin-install`, a pending
`plugin-uninstall`, a pending `email-confirm`, and a `credential` form carrying an
engine `request`. A *completed* install, uninstall or send was exempt, on the
grounds that a receipt is a real destination.

Its stated reason was that these forms are backed by an engine-staged request
whose id "is dead the moment the page reloads". Restoring one would show a form
whose Confirm resolves nothing.

A user reported the consequence. They navigated back to a pending email confirm
and landed on Settings, Access: the bare panel underneath. `restoreState` still
applies the entry's `menuItem` and `settingsSubview`, so nulling the overlay
exposes whatever was behind it. A SENT confirm navigated correctly, which is the
`sentAt` carve-out showing through.

The guard was written for reload and reached four call sites: `ensureInitialized`,
`navBack`, `navForward` and `navGoTo`. So the obvious fix was to scope it to
reload. Checking the premise first showed there was nothing to scope.

## Decision

`isTransientForm` is deleted, along with its call in `restoreState`. Every
panel-nav entry is a regular history destination and restores its overlay on
every path, reload included. A pending form navigates exactly like a confirmed
one.

`suppressUrlPreview` stays. It is a different kind of guard, and the distinction
is the point: it asks a question about the CURRENT device, not about the age of
the entry.

## Rationale

The premise was false in all four cases.

- `email-confirm`: `EmailConfirmRequest` (`store/types.ts`) has no id field. It
  carries the whole draft, and Send posts that draft to `POST /email/send`.
  Nothing is staged on either side.
- `credential`: `CredentialRequest` has no id either. It is a pre-fill descriptor,
  and saving writes a credential row by name and auth type.
- `plugin-install` / `plugin-uninstall`: these DO carry an id, but the staged
  entry lives in `pending_installs` / `pending_uninstalls`, in-memory maps on the
  ENGINE (`engine_impl/construction.rs`), confirmed by
  `POST /api/v1/plugins/install/:install_id/confirm`. A browser reload does not
  touch the engine. Only an engine restart or the TTL sweep clears them.

So no staged request dies with the page, and "transient" described nothing real.
The pending-versus-completed split the guard drew was a distinction without a
mechanism behind it.

Deleting beats scoping for a second reason. A guard applied on a path it was not
written for is exactly the failure here, and a parameter keeps that shape alive:
the next call site added to `restoreState` has to know which value to pass, and
guessing wrong reproduces the bug silently.

## Consequences

- Back, forward and the nav-history popover return the user to a pending confirm,
  which is what the bug report asked for.
- A reload also restores a pending form. That is a deliberate widening, not a side
  effect: the request behind it is still resolvable.
- A plugin request the engine's TTL sweep already reaped now shows its form again,
  and Confirm fails at the engine. That is the same failure any stale confirm
  gives, and the engine reports it. A client-side expiry is not added, because the
  client holds no staleness signal it could key one off.
- `suppressUrlPreview` is now the only overlay guard in `restoreState`, so the
  comment beside it describes one rule rather than two.

## Alternatives considered

**Scope the guard to the reload path**, with an explicit parameter set only by
`ensureInitialized`. This was the first direction and it is what the guard's own
doc comment implies. Rejected once the premise turned out to be false: it
preserves a distinction that describes nothing, and it leaves a parameter every
future call site must get right.

**Sanitize the persisted stack at load**, stripping the overlay off a transient
entry and collapsing the neighbours that then compare equal. Drafted and rejected.
It carries cursor arithmetic, a never-empty invariant and a collapse rule, all in
service of the same false premise. It also still throws away a live form.

**Drop transient entries from the stack at load**, adjusting the cursor for
anything removed at or before it. Rejected for the same reason, and it is worse
than stripping: the user loses the panel they were on, not just the form.

**Keep the guard and add an expiry probe**, asking the engine whether a staged
plugin request is still live before restoring. Rejected as cost with no return.
Two of the four families have nothing to probe, and the failure it would prevent
is already reported by the engine at Confirm time.
