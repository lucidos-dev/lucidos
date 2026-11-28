# 0637: Providers is a Settings page of its own, a settings link takes a fragment and navigate_ui reuses its fragment argument for it, and Manage models folds unservable models into Needs a provider with Retired winning

- **Status**: Accepted
- **Date**: 2026-11-09

## Context

Settings → Models had grown into two pages in one. The top half picks models.
The bottom half set up eleven provider blocks, then listed every registry model,
including the many that no configured provider can serve. A user with one key
scrolled past a dozen rows that could never run.

Three loose ends came with it. Every provider anchor was named `models:<x>`
after the page it no longer belongs on. A settings link could open a page but
not a place on it. The copy named the old location in about forty strings.

## Decision

1. **Providers is a Settings page**, `settings_view` `providers`, first in the
   Assistant group. It has two sections: "Model providers" and "System One".
2. **A provider's anchor is `providers:<ProviderKind value>`**, derived by
   `providerAnchor()`. A `RETIRED_SETTINGS_ANCHORS` map sends every old
   `models:*` id to its new one, so a persisted Search recent still lands.
3. **A settings link takes a fragment.** `settings:<view>#<name>` opens the page
   and scrolls to the anchor `<view>:<name>`. `navigate_ui` reuses its existing
   `fragment` argument for it. A fragment that names nothing lands on the page
   and toasts.
4. **Manage models lists three groups.** Servable models stay in the main list.
   A model with no configured route folds into a closed "Needs a provider (N)"
   group, whose rows link to the provider blocks they need. Retired wins: a
   retired model is never under Needs a provider, whatever its routes.
5. **The path in copy has one definition per language.** The frontend reads
   `PROVIDERS_SETTINGS_PATH` and the engine a `providers_settings!()` macro. A
   retired-terms row guards prose.

## Rationale

- A provider is the thing that makes a model reachable, so it reads first in
  setup order and earns a page rather than a section.
- Renaming the anchors keeps a name true. A `models:xai` anchor on the
  Providers page would mislead the next reader and the engine LLM alike.
- The fragment grammar reuses a field that already means "a place inside the
  target" for apps. A new field would widen the `NavigationRequested` payload
  and the notification shape for no gain. ADR 0048 governs deep links, and this
  keeps its single link shape.
- "Needs a provider" is a call to action. It lists only models worth acting
  on, so a retired model, which the router replaces anyway (ADR 0519), stays out.

## Consequences

- Old `settings_view: 'models'` payloads and notification taps still land on
  Models. Old anchors redirect.
- A fragment resolves to `<view>:<name>`. An anchor that breaks that prefix,
  such as `command-safety:*` on Permissions, is not reachable by fragment.
- `configured_providers` being absent (an older engine) means no Needs-a-provider
  group and an unfiltered Add Model list. Nothing goes spuriously empty.
- The link row counts model providers only, since System One providers serve no
  registry model (ADR 0220).

## Alternatives considered

- **Keep Providers on Models, collapse it.** Cheaper, but the page still mixes
  two jobs, and a collapsed block hides the first thing a new user needs.
- **A new `settings_anchor` field on `navigate_ui`.** Explicit, but it widens
  two wire shapes where `fragment` already fits.
- **Hide unservable models entirely.** Loses the path from a model to the key it
  needs. The closed group keeps the rows and adds the link.
- **Show retired models under Needs a provider too.** Advises setting up a
  provider to reach a model the router already replaces.
