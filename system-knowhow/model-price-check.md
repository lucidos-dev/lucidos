---
name: Model price check
description: Checking model prices against the providers' pages and writing dated cards to the price table's override. Load when Keep prices current fires or prices need checking.
---

# Model price check

The *price table* is what each model charges per million tokens, card by dated
card. Lucidos ships cards and refreshes them with each release. A workspace adds
or corrects cards in its override, `data/config/model-prices.json`, merged card
by card. Every estimate reads the merged table: the cache keep-alive, the
handoff estimate, the intelligence ladder's cost band, usage telemetry and the
Token Cost app.

This recipe keeps the override current from the providers' own pages. Change a
price at once, and report it after. The user does not approve each card first.

## Read the table

Run `lucidos model-prices`. It prints the merged table as JSON:

- `models.<id>` is a list of cards, oldest first, each with its `origin`:
  `shipped` or `override`.
- `checked` is the day the prices in force were last checked.
- `override.error` says why Lucidos ignored the override. Fix that first: until
  then, none of its cards count.

## The override file

Write only the cards that differ from the shipped ones. A card the shipped
table already holds stays out of the override, so a later release can still
correct it. The file's shape:

```json
{
  "checked": "YYYY-MM-DD",
  "currency": "USD",
  "sources": { "anthropic": "<url>", "openai": "<url>" },
  "failures": { "<provider>": { "count": 0, "last_ok": "YYYY-MM-DD" } },
  "models": { "<model id>": [ { "<card>": "..." } ] }
}
```

A card:

- `from` is the first day it is in force (YYYY-MM-DD, local time). A card
  without `from` is in force since the beginning. The next card ends it.
- `uncached_in`, `cache_write`, `cache_read` and `out` are rates per million
  tokens. `cache_write` is the 5-minute write. `cache_write_1h` is the 1-hour
  write, and defaults to `cache_write`.
- `long` is the long-context tier: `threshold_tokens` plus its own rates. A
  call is long only when its prompt is OVER the threshold. Add
  `"inclusive": true` only when the provider bills "at or over" the threshold.
- `flex` is OpenAI's Flex tier, with its own rates. Add it for every OpenAI
  model the Flex table lists.
- `per_minute` prices a model billed by time, such as a realtime model.
- `source` is required: the URL, the quoted sentence, and "checked <date>".

An override card replaces the shipped card from the same day, and joins the
list otherwise. Lucidos ignores the whole override when it has another
currency, two cards for one model on one day, or a negative rate. So read
`override.error` again after every write.

## Sources to read

Read the provider's own page. Never trust a blog post or an aggregator over
it. Cite a community source only when the provider publishes nothing, and say
so in the card's `source`.

| Provider | Price page | Change dates |
|---|---|---|
| Anthropic | https://platform.claude.com/docs/en/about-claude/pricing | https://platform.claude.com/docs/en/release-notes/api |
| OpenAI | https://developers.openai.com/api/docs/pricing, plus the per-model page | https://developers.openai.com/api/docs/changelog |
| Google | https://ai.google.dev/gemini-api/docs/pricing | the same page, and the Vertex release notes |
| xAI | https://docs.x.ai/developers/pricing | the same page |
| OpenRouter | `proxy_request(name: 'openrouter', path: '/models')`, where `pricing` is per token | none |
| TypeSafe | https://docs.typesafe.ai/models.md | none |

Also read each provider's deprecation page, for retired models and for models
it now routes to a newer one. Store the URLs you used in `sources`. A provider
whose models this workspace never calls needs no reading.

### How to fetch a page

1. **Download it with `http_request`**, method `GET`, with `temp_path` set to a
   bare file name such as `pricing-anthropic.html`. The tool saves the page
   under `.lucidos/tmp/` and reports the path.
2. **Parse the saved file with `run_python`.** Strip the HTML and pull out the
   rate rows. The Python makes no network call.

Do not scrape with `urllib` or `curl` inside `run_python` or `run_bash`: the
command guard reviews those calls and can block them. Use `web_search` only to
find a page whose URL moved. Use `browser_open` only for a page that needs
JavaScript to show its table.

A price check only reads. If the command guard blocks a call anyway, do not
ask for a side-effect permission. Record the source as failed instead.

## Which models to check

Check every model the table prices, plus every model this workspace used in
the last 7 days:

```bash
psql "$LUCIDOS_DATABASE_URL" -Atc "SELECT DISTINCT payload->>'model' FROM events
  WHERE event_type = 'ContextCaptured' AND created > now() - interval '7 days'"
```

Strip a `[1m]` suffix and a `vendor/` prefix before you look a model up. A
dated id (`claude-haiku-4-5-20251001`) shares its undated form's cards. Skip
free, local and stealth models (`*-free`, a local Ollama model, `stealth/*`):
they have no price page.

## The check

1. Compare the card in force today with the provider's current rates.
2. Add a card for a used model that has none.
3. Check that a card dated in the future (an announced change) still matches
   the announcement.
4. Check promos with an end date. A provider may say a price holds "at least
   through" a date. Check again the day after, and add a dated card if the
   price changed.

## How to write a change

- **Add a card. Never edit an old one.** Set `from` to the day the change took
  effect, from the provider's changelog. Past days then keep the old price.
- **When the effective date is not published,** use today, and say in `source`
  that the real date is unknown. Never invent a date.
- **When a past card is wrong,** fix it and quote the evidence. That reprices
  history, so the notification says so and gives the size.
- **A retired model keeps its cards,** because history needs them. When the
  provider routes it to another model, say which one in the notification.
- **A new model** gets one undated card if it always had this price, or a card
  dated from its launch day.

Write the whole file with `run_python`: `json.load` the override (or start
from the shape above), change it, and `json.dump(..., indent=2,
ensure_ascii=False)` to a temp file with a trailing newline. Then save it with
`lucidos data write config/model-prices.json --from <temp file>`, which also
commits it. Set `checked` to today whenever every source read.

## When a source cannot be read

Do not guess. Add one to `failures.<provider>.count`, and reset it to 0 with
`last_ok` set to today when the page reads again. Tell the user only when the
same provider has failed 3 checks in a row.

## Notify

Notify only when a price changed, or when a source failed 3 checks in a row:

- `send_notification`, title "Model prices changed", with one `• ` line per
  change: the model, the old rate, the new rate and the `from` day. Add a line
  when history was repriced, with the size of the difference.
- Then `request_read`, so the run waits under Review.

When nothing changed, do neither. The only write is then `checked`.

## Keep prices current

Settings → Intelligence shows "Prices checked <date>", and its **Keep prices
current** button creates an intent trigger that runs this check daily or
weekly. Lucidos offers that trigger and never creates it unasked. If the user
asks for a check schedule elsewhere, point them at that button. The intent says
what the user wants, so it never repeats this recipe.
