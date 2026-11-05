# What the tools array and the system prompt actually cost

Investigation only. Read-only analysis of the `dev` workspace event store and
the engine source. No engine behaviour was changed.

ADR 0085 ends with an open item: "The two largest fixed blocks were never
examined. The tools array is 113 schemas on every request of every thread, and
the system prompt is about 22.9k tokens. Both are read at 0.1x every round and
neither came up." This is that examination.

- **Window**: 7 days to 2026-08-18, `main_llm` producer, `claude-opus-5` only.
- **Population**: 3,437 `ContextCaptured` calls, 268 turns, 84 threads.
  Structural claims are re-checked over 30 days, 1,423 turns.
- **Prior art**: ADR 0084, ADR 0085, ADR 0086, and
  `data/artifacts/context-economics-investigation.md` in the `dev` workspace.

## Executive summary

- **The two blocks are 47,912 tokens of a 113,899-token mean prompt, so 42.1%
  of every request, and 32.2% of the 7-day Opus bill.** ADR 0085 was right that
  this is the largest unpriced item. It is larger than the 25.0% turn boundary.
- **Exact sizes, measured from provider accounting rather than estimated.**
  The tools array is **27,175 tokens**. The system block is **21,668 tokens**.
  Both are single exact numbers per engine build, not distributions.
- **The ~9,200 unmatched tools tokens are not real.** They are the residual of
  a three-way arithmetic split, and a bimodal mean subtracted from a mode. Over
  30 days and 1,423 turn boundaries, **not one** cache read landed between zero
  and the full tools tier. The tools prefix is never partially matched.
- **The money is the miss, not the size.** Of the two blocks' $147.52 over 7
  days, $82.19 is the floor a perfect cache would still pay and $65.33 is the
  miss surcharge. **58.6% of turn boundaries read nothing at all**, including a
  tools array that is byte-identical across every thread on the build.
- **Every thread carries the identical array.** 3,639 calls, 84 threads, 72
  schemas, one order, one hash. Chat and trigger threads do not differ, and a
  coding-agent thread's Claude Code calls carry Claude Code's own tools instead.
- **The count in ADR 0085 is wrong.** It is 72 schemas on the wire, not 113.
- **Prose trimming is not defensible on this evidence.** The system prompt
  shares 0.36% of its 8-word shingles with the tool schemas and 0.30% of its
  20-word shingles with the whole engine-shipped knowhow corpus. There is
  almost nothing said twice.
- **Two values violate ADR 0084 today**, and neither trips its guard: the
  client-URL sentence and the `ENGINE BUILD` section.

---

## Method

### Where the numbers come from

`ContextCaptured` carries three things this investigation needs, and the
existing artifact did not use any of them.

1. `payload.tools`, the **exact tool-name array** sent on that call, in order.
2. A `Tool Definitions (N)` section whose declared size is the engine's own
   accounting of the schemas.
3. A `System Instructions` section whose declared size is the assembled system
   block.

The engine's tool accounting is not the wire form.
`engine::context::tool_definitions_chars` sums
`name + description + parameters` and adds a flat
`TOOL_DEF_OVERHEAD_CHARS = 100` per tool. The wire form is
`serde_json::to_string` of the definition, which adds 43 characters of JSON
scaffolding. So `wire = captured - 100n + 43n`, and for the current 72-tool
array that is `73,560 - 7,200 + 3,096 = 69,456` characters.

The 2026-08-17 cache probe measured the serialized `ClaudeTool` array at 69,750
bytes on a neighbouring build. The two reconcile once `input_schema` (two
characters longer than `parameters`), the array brackets and the trailing
`cache_control` marker are added.

### Characters to tokens, measured rather than assumed

Anthropic reports `cache_read_tokens` for a prefix that ends at a
`cache_control` marker. The first marker sits on `tools[-1]`
(`anthropic_wire.rs`, `apply_cache_control_to_last_tool`). So a read that stops
at the tools tier IS the tools tier's token count, stated by the provider.

Bucketing every 30-day call whose read lands in that band, by the build's own
tool character count:

| tool chars | calls | read low | read high | chars/token |
|---:|---:|---:|---:|---:|
| 70,943 | 16 | 26,156 | 26,156 | 2.5554 |
| 71,140 | 5 | 26,238 | 26,238 | 2.5549 |
| 71,537 | 5 | 26,415 | 26,415 | 2.5528 |
| 71,548 | 18 | 26,418 | 26,418 | 2.5530 |
| 72,832 | 4 | 26,866 | 26,866 | 2.5582 |
| 73,073 | 11 | 26,957 | 26,957 | 2.5585 |
| 73,077 | 39 | 26,947 | 26,947 | 2.5596 |
| 73,085 | 4 | 26,963 | 26,963 | 2.5584 |
| 73,144 | 49 | 26,984 | 26,985 | 2.5585 |
| 73,150 | 14 | 26,989 | 26,989 | 2.5583 |
| 73,158 | 4 | 27,009 | 27,009 | 2.5567 |
| 73,161 | 14 | 26,997 | 26,997 | 2.5580 |
| 73,481 | 2 | 27,145 | 27,145 | 2.5558 |
| 73,560 | 8 | 27,175 | 27,175 | 2.5559 |
| 73,742 | 18 | 27,068 | 27,068 | 2.5727 |

Every build reads a **single exact value**, low equal to high. The ratio holds
at **2.5567 wire characters per token**, spread 2.553 to 2.573.

The system tier follows by subtraction on a call that read both tiers. On
2026-08-18, four calls read 48,843 with the tools tier at exactly 27,175:
`48,843 - 27,175 = 21,668` tokens for 58,453 characters, or **2.698
characters per token**. Denser prose tokenizes worse than JSON, as expected.

Both ratios are the provider's own arithmetic. Nothing here is an estimate.

---

## Part 1. The tools array

### It is 72 schemas, identical everywhere

Over 7 days, every `main_llm` call in the workspace presented the same array:

| tools | calls | threads | distinct orderings |
|---:|---:|---:|---:|
| 72 | 3,639 | 84 | 1 |

One shape hash across 3,639 calls. Chat threads, trigger threads and the
Lucidos-agent calls that run on coding-agent threads all carry it. A
coding-agent thread also produces 83,940 `claude_code` captures over 30 days.
Those carry Claude Code's own tools, which Lucidos neither builds nor pays for
here.

ADR 0085's "113 schemas" does not reproduce. The wire array is 57 flat schemas,
14 grouped manifest schemas and `generate_image` when an image provider is
configured, which is 72.

### Per-tool cost

Measured on the current tree with
`cargo test -p lucidos-engine --lib print_full_tool_schema_ranking -- --ignored --nocapture`,
which is an existing diagnostic in `system_prompt.rs`. It reports 71 tools and
68,122 characters, excluding the conditional `generate_image`. The live build
adds that one for 69,456 characters and 27,175 tokens.

| # | tool | wire chars | tokens | cumulative |
|---:|---|---:|---:|---:|
| 1 | `triggers` | 2,904 | 1,136 | 4.3% |
| 2 | `run_coding_agent` | 2,553 | 999 | 8.0% |
| 3 | `await_event` | 2,547 | 996 | 11.7% |
| 4 | `navigate_ui` | 2,244 | 878 | 15.0% |
| 5 | `request_credential` | 2,067 | 808 | 18.1% |
| 6 | `ask_user_question` | 1,865 | 729 | 20.8% |
| 7 | `threads` | 1,824 | 713 | 23.5% |
| 8 | `follow_up_child_thread` | 1,819 | 711 | 26.2% |
| 9 | `memory` | 1,744 | 682 | 28.7% |
| 10 | `events` | 1,617 | 632 | 31.1% |
| 11 | `bash_output` | 1,486 | 581 | 33.3% |
| 12 | `thread_queue` | 1,485 | 581 | 35.5% |
| 13 | `connect_oauth_account` | 1,483 | 580 | 37.6% |
| 14 | `manage_models` | 1,474 | 577 | 39.8% |
| 15 | `edit_file` | 1,426 | 558 | 41.9% |
| 16 | `send_notification` | 1,389 | 543 | 43.9% |
| 17 | `plugins` | 1,335 | 522 | 45.9% |
| 18 | `run_thread` | 1,263 | 494 | 47.7% |
| 19 | `trigger_groups` | 1,257 | 492 | 49.6% |
| 20 | `mcp` | 1,234 | 483 | 51.4% |
| 21 | `read_file` | 1,203 | 471 | 53.2% |
| 22 | `run_python_background` | 1,197 | 468 | 54.9% |
| 23 | `configure_email` | 1,180 | 462 | 56.7% |
| 24 | `run_python` | 1,172 | 458 | 58.4% |
| 25 | `git_clone` | 1,132 | 443 | 60.0% |
| 26 | `create_app` | 1,124 | 440 | 61.7% |
| 27 | `todo_write` | 1,063 | 416 | 63.2% |
| 28 | `proxy_request` | 1,053 | 412 | 64.8% |
| 29 | `run_bash` | 1,013 | 396 | 66.3% |
| 30 | `run_bash_background` | 1,004 | 393 | 67.8% |
| 31 | `grep_files` | 991 | 388 | 69.2% |
| 32 | `http_request` | 938 | 367 | 70.6% |
| 33 | `env_vars` | 936 | 366 | 72.0% |
| 34 | `send_email` | 906 | 354 | 73.3% |
| 35 | `cancel_event_wait` | 902 | 353 | 74.6% |
| 36 | `preferences` | 890 | 348 | 75.9% |
| 37 | `manage_repositories` | 827 | 323 | 77.1% |
| 38 | `write_file` | 794 | 311 | 78.3% |
| 39 | `changes` | 777 | 304 | 79.4% |
| 40 | `notifications` | 774 | 303 | 80.6% |
| 41 | `save_email_attachment` | 755 | 295 | 81.7% |
| 42 | `copy_file` | 614 | 240 | 82.6% |
| 43 | `read_emails` | 610 | 239 | 83.5% |
| 44 | `browser_screenshot` | 588 | 230 | 84.3% |
| 45 | `dismiss_from_context` | 570 | 223 | 85.2% |
| 46 | `view_image` | 559 | 219 | 86.0% |
| 47 | `import_file` | 543 | 212 | 86.8% |
| 48 | `browser_open` | 542 | 212 | 87.6% |
| 49 | `refresh_app` | 517 | 202 | 88.4% |
| 50 | `delete_file` | 513 | 201 | 89.1% |
| 51 | `execute_intent` | 470 | 184 | 89.8% |
| 52 | `browser_type` | 463 | 181 | 90.5% |
| 53 | `save_thread_image` | 462 | 181 | 91.2% |
| 54 | `browser_extract` | 456 | 178 | 91.8% |
| 55 | `fetch_news` | 451 | 176 | 92.5% |
| 56 | `list_event_waits` | 440 | 172 | 93.1% |
| 57 | `get_backup_status` | 438 | 171 | 93.8% |
| 58 | `glob_files` | 435 | 170 | 94.4% |
| 59 | `reload_proxy_modules` | 419 | 164 | 95.0% |
| 60 | `web_search` | 397 | 155 | 95.6% |
| 61 | `read_email` | 393 | 154 | 96.2% |
| 62 | `load_knowhow` | 366 | 143 | 96.7% |
| 63 | `browser_click` | 338 | 132 | 97.2% |
| 64 | `bash_kill` | 322 | 126 | 97.7% |
| 65 | `capture_app` | 304 | 119 | 98.1% |
| 66 | `browser_eval` | 280 | 110 | 98.6% |
| 67 | `browser_forget_login` | 264 | 103 | 98.9% |
| 68 | `browser_clear_data` | 213 | 83 | 99.3% |
| 69 | `list_files` | 198 | 77 | 99.5% |
| 70 | `browser_close` | 171 | 67 | 99.8% |
| 71 | `list_apps` | 139 | 54 | 100.0% |

**The array has no fat head.** The largest single schema is 4.3% of it, and the
top ten are 31.1%. Twenty schemas carry half. This is not a distribution where
trimming three tools moves the number.

By family:

| family | tools | chars | tokens | share |
|---|---:|---:|---:|---:|
| ungrouped remainder | 25 | 26,564 | 10,390 | 39.0% |
| grouped manifest | 14 | 19,078 | 7,462 | 28.0% |
| file | 9 | 6,717 | 2,627 | 9.9% |
| exec | 6 | 6,194 | 2,423 | 9.1% |
| email | 5 | 3,844 | 1,504 | 5.6% |
| browser | 9 | 3,315 | 1,297 | 4.9% |
| proxy and http | 3 | 2,410 | 943 | 3.5% |

### What actually gets called

`ToolCalled` over 90 days, joined to the offered array. 61 of the 72 were
called at least once. The eleven never called:

`browser_clear_data`, `browser_forget_login`, `browser_type`,
`configure_email`, `execute_intent`, `fetch_news`, `generate_image`,
`read_email`, `reload_proxy_modules`, `save_email_attachment`, `send_email`.

Those eleven are 6,432 characters, 2,516 tokens, 9.3% of the array.

**Two distinct reasons hide inside that list, and only one is about value.**
The workspace has **zero email accounts** and **zero intents**, so the five
email schemas and `execute_intent` cannot be used here at all. That is 4,314
characters of capability the workspace does not have. `generate_image` is
already gated on an image provider being configured, so the pattern exists.

Concentration, over 32,452 chat tool calls in 90 days:

| resident core | threads needing a fetch | of threads | distinct fetches |
|---:|---:|---:|---:|
| top 10 | 896 | 953 | 3,155 |
| top 15 | 831 | 953 | 2,212 |
| top 20 | 645 | 953 | 1,419 |
| top 25 | 398 | 953 | 877 |
| top 30 | 345 | 953 | 662 |
| top 40 | 197 | 953 | 296 |

The head is steep and the tail is wide. `run_bash` alone is 35.6% of calls, and
the top ten are 79.8%. But the tail is not dead weight: `send_notification` is
214 calls across **207 threads**, so it is used once by almost every thread
that uses it at all. Hiding the tail therefore touches most threads.

---

## Part 2. The system prompt

### Assembled size, reconciled section by section

`build_chat_system_prompt` appends nine pieces in a fixed order. Sizes below
come from the source constants, the existing
`always_loaded_context_stays_under_budget` diagnostic, and the live workspace's
own apps and knowhow.

| section | chars | tokens | shape |
|---|---:|---:|---|
| Workspace identity: name, path, timezone rules, language | 1,506 | 558 | workspace and preference |
| `SYSTEM_PROMPT_BASE` plus 12 spliced rule constants | 30,044 | 11,136 | constant, two install variants |
| `ENGINE BUILD` | 327 to 374 | 121 to 139 | dev install only, live build state |
| Client URL sentence | 137 | 51 | last observed request origin |
| Available Apps, 24 apps | 5,367 | 1,989 | workspace |
| Available Intents, 0 intents | 0 | 0 | workspace |
| Know-how routing, 19 workspace and 3 app docs | 10,416 | 3,861 | workspace |
| System Knowhow routing list, 26 engine docs | 9,315 | 3,453 | constant everywhere |
| Images | 995, plus 288 with an image provider | 476 | constant, one conditional clause |
| **Total** | **58,442** | **21,663** | |

The measured `System Instructions` char count on 2026-08-18 is **58,453**, so
the reconstruction is short by 11 characters out of 58,442. The token total
derived from the provider is 21,668 against 21,663 here.

Inside `SYSTEM_PROMPT_BASE` and its rules:

| constant | chars |
|---|---:|
| `SYSTEM_PROMPT_BASE` template | 18,067 |
| `ASK_USER_QUESTION_RULE` | 2,934 |
| `NAMES_NOT_IDS_RULE` | 1,472 |
| `NO_LUCIDOS_SOURCE_SECTION` (packaged install only) | 1,336 |
| `SETUP_INTERVIEW_RULE` | 1,048 |
| `WORKSPACE_ASSETS_KNOWHOW_RULE` | 1,025 |
| `NO_IMPERSONATION_RULE` | 893 |
| `APPLY_VERIFY_RULE` | 883 |
| `TRIGGER_VS_EVENT_WAIT_RULE` | 876 |
| `REPEATED_ACTION_RULE` | 782 |
| `APPLY_VERIFY_DEV_ADDENDUM` (dev install only) | 692 |
| `ENGINE_RESTART_RULE` | 605 |
| `LOOK_BEFORE_ASSESSING_RULE` | 588 |
| `LUCIDOS_SOURCE_SECTION` (dev install only) | 478 |

A trigger thread additionally carries `TRIGGER_SYSTEM_ADDENDUM` at 855
characters plus a per-trigger knowhow listing, appended last.

### Which parts are constant everywhere

| class | chars | tokens | share |
|---|---:|---:|---:|
| Constant on every workspace on earth | 40,642 | 15,064 | 69.5% |
| Workspace-shaped (apps, knowhow, name, path) | 15,805 | 5,858 | 27.0% |
| Preference and device shaped (timezone, language, client URL) | 755 | 280 | 1.3% |
| Install and build state (`ENGINE BUILD`) | 374 | 139 | 0.6% |

Two thirds of the system block is identical for every Lucidos install running
the build. The workspace-shaped quarter is the apps list and the knowhow
routing list, which ADR 0086 has already ruled on and this investigation does
not touch.

### It does not repeat itself

A rule stated in the system prompt and again in a tool schema is billed twice
on every request. Measured with word shingles, where a match at length 8 is a
restated clause rather than a shared idiom:

| shingle length | in prompt | in flat tool schemas | shared | share of prompt |
|---:|---:|---:|---:|---:|
| 8 | 5,055 | 4,296 | 18 | 0.36% |
| 12 | 5,055 | 4,309 | 4 | 0.08% |
| 20 | 5,047 | 4,325 | 0 | 0.00% |

All eighteen shared runs are one topic: the `ask_user_question` ban on an
"Other" option, which the tree mirrors on purpose and pins with a test in each
place.

Against the whole engine-shipped knowhow corpus, 26 files and 1,047,678 bytes:

| shingle length | shared | share of prompt |
|---:|---:|---:|
| 8 | 101 | 2.00% |
| 12 | 47 | 0.93% |
| 20 | 15 | 0.30% |

Concentrated in `setup-interview.md` (45 shared 8-grams), `lucidos-cli.md`
(20), `triggers.md` (18) and `running-python.md` (12). That is the routing rule
and its destination sharing vocabulary, which is what routing looks like. The
whole `SETUP_INTERVIEW_RULE` is 1,048 characters.

**There is no measurable duplication to reclaim.**

---

## Part 3. What the two blocks cost

### The bill

Seven days, `claude-opus-5`, `main_llm`, first-party Anthropic list rates
applied to real Vertex token counts, as in the prior artifact. A tier counts as
READ when the reported `cache_read` reaches it, and as WRITTEN otherwise.

| | calls | total | tools block | system block |
|---|---:|---:|---:|---:|
| chat turns | 3,192 | $434.79 | $70.60 | $67.79 |
| trigger turns | 245 | $24.33 | $5.02 | $4.11 |
| **all** | **3,437** | **$459.12** | **$75.62** | **$71.90** |

| | calls | total | tools | system | tools read | system read |
|---|---:|---:|---:|---:|---:|---:|
| first of turn | 268 | $117.57 | $27.85 | $33.73 | 41.8% | 4.1% |
| later rounds | 3,169 | $341.55 | $47.77 | $38.17 | 99.0% | 98.7% |

**The two blocks are $147.52 of $459.12, so 32.2% of the 7-day Opus bill.** For
comparison, the 30-day artifact put the whole turn boundary at 25.0%.

### The floor

| quantity | value |
|---|---:|
| Mean tools tier | 27,010 tokens |
| Mean system tier | 20,902 tokens |
| Mean prompt | 113,899 tokens |
| The two blocks as a share of the prompt | **42.1%** |
| Cost if every call read both at 0.1x | **$82.19** |
| Actual cost | $147.52 |
| Miss surcharge | $65.33 |

$82.19 is 17.9% of the bill and **no cache improvement can touch it**. It comes
down only if the blocks get smaller. The other $65.33 is the miss, and it comes
down only if the cache stops missing.

Marginal value of size, at the observed hit rates:

| cut | worth per week |
|---|---:|
| 1,000 characters off the tools array | $1.09 |
| 1,000 characters off the system prompt | $1.27 |

So the whole email family, 3,844 characters, is $4.19 a week here. A 20% cut of
the tools array would be $15.20 a week. This is one heavy dev workspace at
$65.59 a day, so treat the shares as portable and the dollars as local.

### ADR 0084 landed inside the window

The clock fix merged at 16:29 on 2026-08-17, so 6 of the 7 days are pre-fix.

| | calls | total | tools | system | tools read | system read |
|---|---:|---:|---:|---:|---:|---:|
| pre 0084 | 3,306 | $444.90 | $72.91 | $69.62 | 94.5% | 91.2% |
| post 0084 | 125 | $13.51 | $2.64 | $2.23 | 95.2% | 94.4% |

At turn boundaries specifically, post-fix, 5 of 9 read tools and system
together against 3.7% pre-fix. The direction matches 0084's prediction. **The
sample is 9 boundaries and settles nothing.**

---

## Part 4. The ~9,200 unmatched tools tokens

**They are not real.** Two separate arithmetic artifacts produced them, and the
data contradicts both.

### The tools tier is never partially matched

Every first-of-turn call over 30 days, classified against the tier sizes of its
own build:

| where the read landed | boundaries | share |
|---|---:|---:|
| nothing | 808 | 56.8% |
| **strictly between zero and the tools tier** | **0** | **0.0%** |
| the tools tier | 558 | 39.2% |
| tools and system | 56 | 3.9% |
| into the messages tier | 1 | 0.1% |

Zero. Over 1,423 turn boundaries and 15 engine builds, no read ever stopped
part way through the tools array. The plateau table above shows why: each build
reads one exact value, low equal to high. A prefix that diverged inside the
array would produce a spread, and there is none.

### The 22,659 was a mixture, not a shortfall

The prior artifact read a residue out of a `global_warm` bucket whose mean read
was 22,659 against a 27,175-token tier. Reproducing that bucketing over 30 days:

| global warm | boundaries | mean read | read nothing | mean read when non-zero |
|---|---:|---:|---:|---:|
| no | 495 | 2,010 | 467 (94.3%) | 35,538 |
| yes | 928 | 22,506 | 341 (36.7%) | 35,581 |

The mean of 22,506 reproduces. It is 36.7% zeros mixed with reads that average
**35,581, which is above the tools tier, not below it**. Subtracting a bimodal
mean from a mode measures the zeros, not a partial match.

### The $0.058 figure came from a residual

The artifact's per-boundary split priced the residue at $0.058, which is 9,280
tokens at $6.25/MTok. But its own direct measurement, 27,175 minus 22,659, is
4,516 tokens and $0.028. The 9,200 is what is left after subtracting an
estimated clock share and an estimated messages share from the 67,589-token
boundary write. It is a remainder, and remainders absorb every error in the
terms above them.

The registry entry already says "Reads are all-or-nothing. Nothing lands
between zero and the 22.4k tools block." That sentence and the 9,200 cannot
both be true, and the data says the sentence is right.

### What the real question is

Removing the 9,200 does not close the investigation. It sharpens it. The
remaining fact is larger and stranger.

**58.6% of turn boundaries read nothing at all.** The tools array they failed
to match is byte-identical across every thread, every turn and every workspace
on that engine build. There is no content explanation available. The tools tier
cannot have diverged, because a divergence would show as a partial read and
partial reads do not occur.

---

## Part 5. Does anything violate ADR 0084

ADR 0084 rule 1: "The system block holds nothing that varies per turn or per
thread. It is a function of workspace state and preferences, and of nothing
else." Two sections fail that test, and neither trips
`two_threads_in_one_workspace_share_the_system_block`, because that guard
compares two threads at one moment.

### The `ENGINE BUILD` section, 327 to 374 chars

Its own doc comment says "rebuilt every turn, never stale". It is a function of
live build state, which is neither workspace state nor a preference. Its four
states have four lengths:

| state | chars |
|---:|---|
| `update_available` | 327 |
| `source_behind_head` | 359 |
| `current` | 371 |
| `rebuild_wedged` | 374 |

Pairwise deltas are 3, 12, 15, 32, 44 and 47 characters. In the 7-day data, two
threads in one hour and one build repeatedly disagree on system block length by
**exactly 44 characters**. That is `update_available` against `current`, so the
section is flipping mid-window.

A flip costs one system-tier rewrite for the whole workspace,
`21,668 x ($6.25 - $0.50)/MTok = $0.125`. Within-thread transitions moved the
system block by 100 characters or less at 28 of 277 turn transitions, 10.1%.

### The client URL sentence, 137 chars

`build_chat_system_prompt` reads `self.frontend_origin`, a runtime `Mutex` set
from the last observed request origin. That is per-request state, not workspace
state. A user reaching the same workspace from two origins rewrites the whole
system tier when the origin changes.

### How stable the block actually is

| turn to turn, within one thread | count | share |
|---|---:|---:|
| unchanged length | 151 | 54.5% |
| first turn of the thread | 84 | 30.3% |
| moved by 100 chars or less | 28 | 10.1% |
| moved by 101 to 2,000 chars | 14 | 5.1% |

Equal length is not equal content, as ADR 0084 established with the clock. So
54.5% is an upper bound on stability, not a measurement of it.

---

## Reproducing this

Every query is against the `dev` workspace database. The two engine-side
measurements are existing diagnostics, not new code:

```
cargo test -p lucidos-engine --lib print_full_tool_schema_ranking -- --ignored --nocapture
cargo test -p lucidos-engine --lib always_loaded_context_stays_under_budget -- --nocapture
```

The tools plateau, which is the load-bearing measurement:

```sql
with s as (
  select e.id,
         (e.payload->'usage'->>'cache_read_tokens')::bigint cr,
         jsonb_array_length(e.payload->'tools') nt,
         max(case when x->>'name' like 'Tool Definitions%'
                  then coalesce(x->>'budget_delta_chars',
                                x->>'char_count')::int end) tool_chars
  from events e, jsonb_array_elements(e.payload->'sections') x
  where e.event_type='ContextCaptured' and e.payload->>'producer'='main_llm'
    and e.payload->>'model' like 'claude-opus-5%'
    and e.created > now() - interval '30 days'
  group by 1,2,3)
select tool_chars, count(*) n, min(cr) lo, max(cr) hi,
       round(avg((tool_chars - nt*100 + nt*43)::numeric / cr), 4) chars_per_token
from s where cr between 25000 and 29500 group by 1 order by 1;
```

## Judgement, as opposed to measurement

Stated separately so a later reader can disagree with the right thing.

- That `wire = captured - 100n + 43n` is the right conversion. It is checked
  against the probe's 69,750 bytes on a neighbouring build, not proved.
- That the eleven never-called tools split into "unusable here" and "unused".
  The email split is certain (zero accounts). `browser_type` at zero calls
  beside `browser_open` at 131 is a judgement.
- That word shingles measure redundancy. They catch restated clauses and miss
  a rule paraphrased in different words.
- That the `ENGINE BUILD` 44-character delta in the data is that section
  flipping. The lengths match exactly and nothing else in the block is known to
  move by 44, but no content was captured to prove it.
- That the shares port to other workspaces while the dollars do not.
