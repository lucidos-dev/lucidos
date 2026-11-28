# 0247: A model registry row carries routes, not one provider

- **Status**: Accepted
- **Date**: 2026-09-22

## Context

Every Claude Opus and Sonnet row in the *model registry* was seeded on `vertex`.
Only the Fable rows used the direct `anthropic` provider. That split was an
artifact of one maintainer's setup, not a product decision.

The damage was silent. `chatModelOptions` filtered the chat picker on the row's
one `provider`. A workspace whose only credential was an Anthropic API key got
the Fable rows and nothing else from the Claude family. That key serves every
one of them.

The schema was what blocked the fix. `models.provider` named THE backend, and
`models.id` was the literal string sent to it. "The same model on two providers"
needed two rows, and two rows means two entries in the picker.

## Decision

A `models` row carries an ordered `routes` list. Each route names a provider,
the id to send it, and that backend's context window. The `provider` and
`context_window` columns retire into it.

`models.preferred_provider` remembers the pick, per model. Resolution is the
preferred route when its provider is configured, else the first configured
route. **An explicit choice is honoured or refused, never substituted.**

## Rationale

One row is one model, which is what the picker shows. Two rows for one model
would have been the smaller change and was rejected outright: the user requires
one entry.

**The route carries its own id** because the same model is not spelled the same
everywhere. OpenRouter prefixes (`anthropic/claude-opus-5-5`). A shared id could
not express that, and it also forces `@default` to be re-spelled before a direct
Anthropic route can exist at all.

**The window rides the route** because it is a property of the request, not of
the model. Take a `[1m]` row reached through a route whose id carries no suffix.
It sent no 1M beta, so budgeting it at 1M would have the packer build a prompt
the backend rejects. The id-shape fallback reads the route's wire id, so an
undeclared route self-corrects.

**Per model, not per account, for the pick.** One global provider preference
means nothing across families that share no backend: picking Grok on xAI would
move Opus off Anthropic. The row already holds every other per-model setting.

**Honoured or refused** because Vertex and Anthropic direct are different
companies, different regions and different data-handling terms. Silently moving
a turn to the other vendor undoes a choice made for residency, with no trace.
The clamp precedent (`clamp_effort` snaps rather than failing) does not carry
here: snapping an effort spends less thought, where substituting a provider
sends the user's data somewhere they declined.

## Consequences

- The picker filters on "any route configured", which is the reported bug fixed.
- A model whose pick is parked stays LISTED, because the provider step is the
  only place to fix it. Hiding it would be a dead end.
- `reasoning_efforts` moves onto the route on `GET /api/v1/models`, since the
  answer depends on the backend: a Claude id offers six on Vertex and four
  through OpenRouter, whose server has no `xhigh`.
- The provider joins the model and the effort wherever they travel together: the
  starter events, per-thread memory, the chat request, the Thread Queue entry,
  and a trigger pin.
- `LlmProvider::chat` takes one `ModelSelection` instead of three model
  arguments. The router hands each leaf the resolved route.
- A CHECK constraint makes an empty route list and a duplicated provider
  unstorable, so a model cannot be silently unreachable.
- OpenRouter routes on Claude rows are expressible and deliberately unseeded.
  The retired Opus 4.x and Sonnet 4.6 rows keep their single Vertex route: their
  direct-API ids were never probed, and seeding one unverified would trade a
  clean "not configured" refusal for a vendor 404.

## Alternatives considered

**An ordered list of provider names, one shared id.** The smallest storage
change. It cannot express OpenRouter at all, and it forces the `@default`
re-spell rather than leaving it a separate decision. Rejected for being wrong
about a case the tree already has.

**One row per backend, collapsed in the picker by a family key.** No new storage
shape, and per-backend window and efforts come free. Rejected because the
model's identity splits: a saved `chat_model` names a ROW, so which backend a
thread runs on stops being stable.

**A `model_routes` table.** Proper constraints and typed columns. Rejected for
two costs: a join on every registry reload, and a second write path through a
store whose design rests on "no caller can skip the event". The route list is
only ever read whole.

**Falling through to the next configured route when a pick is parked.** Matches
the tree's never-dead-end precedent. Rejected: see the rationale above.

**A per-model preference map.** Keeps the registry read-only from the picker.
Rejected because `PrefValue` has no map variant, so it needs a new one plus
catalog validation, or an unvalidated JSON blob riding `Text`.

## Amendment: how the choice is made and remembered

Settled while finishing the change.

- **The provider step shows only where there is a real choice.** That is two
  configured routes, or a route in force that is not configured. A workspace
  with one configured backend picks a model in two steps, as before.
- **Every chat-picker pick remembers the backend on the model's row**, compose
  included. That is the one exception to the rule that a compose pick writes no
  account-wide state, and the user asked for it: the last provider picked is
  remembered per model. The draft or thread that picked also carries its own
  override, so a later pick elsewhere cannot move it. A trigger pin writes only
  the trigger.
- **Thread memory keeps a backend with its model.** It reads the newest pin
  stamped beside the model the turn runs on, so switching models drops it.
  Only a pin is stamped, never a backend the row chose. Removing a credential
  therefore cannot strand a thread that never chose one.
- **A choice is parsed strictly.** An unknown provider name is refused at the
  chat request, the trigger API and the tools, rather than read as a Vertex pin.
  Only a stored route keeps the lenient fallback.
