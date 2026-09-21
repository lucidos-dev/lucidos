---
name: using-jev
description: Ask Jev, TypeSafe's System One model, for a typed judgment instead of prose. Use when a step needs semantic understanding but the answer must be consumed by code: routing, ranking, classifying, extracting, verifying, scoring, or any yes/no a branch depends on. Triggers include "ask Jev", "what does Jev think", "use TypeSafe", "classify these", "score these", "which of these is best". Covers the proxy call, the three question types, and when a judgment beats a prompt.
---

# Using Jev

Jev is TypeSafe's flagship System One model. It answers a question with a typed
value and a probability rather than with prose. Code owns the workflow and Jev
supplies the judgment inside it.

Reach for Jev when a step needs semantic understanding and the answer feeds
code: a branch, a rank, a filter. Reach for a chat model instead when the
output is for a person to read. The same goes for multi-step reasoning, tool
use, or long generation.

Jev cannot see images, run tools, or explain itself. It reads the state you
give it and returns numbers.

## This skill is the coding-agent route

The Lucidos Agent already has Jev as a tool. From a chat or trigger thread it
calls `judge`. That tool takes a state and a set of typed questions, and hands
back every answer with its whole distribution. See *judge tool* in
[`docs/glossary.md`](../../../docs/glossary.md).

A coding-agent session can call no LLM tool, so it POSTs to the endpoint
through the engine proxy. That is what follows. The same call is right for a
`run_python` or `run_bash` script and for a script-type trigger.

The `judge` tool is off whenever TypeSafe is switched off under Settings →
Models → Providers, and the proxy route sits outside that switch. A script
cannot read the switch, since the preference is human-only. So treat a
switched-off provider as a no and do not route around it.

## Setup: there is none

`typesafe` is a **built-in provider proxy**, so it needs no entry in
`data/config/apis.json`. The engine owns the routing and injects the
credential, exactly as it does for `openai` and `anthropic`. See § "Built-in
model-provider proxies" in `system-knowhow/js-sdk.md`.

The one requirement is a stored `typesafe` credential, added under Settings →
Models → Providers, or a `TYPESAFE_API_KEY` in the engine's environment. Check
with `lucidos credentials list`. With neither, the call returns a 404 naming
what to set.

## The call

The built-in base URL is `https://api.typesafe.ai/v1`, so send **`/systemone`**:

```bash
echo '{
  "state": "The payouts have been failing for three days.",
  "model": "jev-latest",
  "questions": {
    "is_urgent": { "type": "noul", "instructions": "Does this convey urgency?" }
  }
}' | lucidos proxy typesafe /systemone -X POST \
       -H "Content-Type: application/json" --data-stdin
```

**Not `/v1/systemone`.** That doubles the prefix and returns `Not Found`, which
reads like a missing credential and is not one. An `apis.json` entry of the
same name overrides the built-in, base URL included. Adding one here buys
nothing and moves the path.

`state` is the content to judge. It takes a string, an object, or an array. Use
named fields when the context has several parts. Point at one from the
instructions with a backticked path, such as `ticket.messages[0].text`.

## The three question types

Pick by what the answer means.

| Need | `type` | Returns |
|---|---|---|
| Whether a condition holds | `noul` | `noul`, a probability from 0 to 1 |
| One option from your set | `choice` | `choice`, plus `probabilities` over every option and a `confidence` |
| A position on an ordered scale | `score` | `score`, which can land between levels, plus `probabilities` and a `confidence` |

A `choice` needs `criteria` mapping each option to a description, up to 255
options. A `score` needs `criteria` as an ordered array of level descriptions,
between two and ten of them. A `noul` may take `criteria` with `true` and
`false` keys saying what each end means.

## Ask everything at once

Questions in one request run in parallel and cannot see each other's answers.
So ask every independent question together, including speculative ones, and let
code use only the branches that apply. Send a second request only when an
earlier answer is needed to fetch evidence or build new state.

Answers come back under the keys you chose. The key itself is never sent to the
model, so the whole meaning has to live in `instructions`.

## Rules that bite

- **Put the full question in `instructions`.** A key named `is_urgent` tells the
  model nothing.
- **Ask one narrow judgment per question.** Split independent dimensions into
  separate questions rather than asking for a verdict on several at once.
- **Include a no-match option** in a `choice` when nothing may fit. The model
  must pick something from the set it is given.
- **`confidence` is not correctness.** It measures how concentrated the
  distribution is. Several acceptable answers spread it, which is fine.
- **A `noul` near 0.5 means undecided**, not medium intensity.
- **Keep the policy in code.** Store the raw judgments. Let weights and
  thresholds live in your own logic, so changing one needs no new call.
- **Report a probability as a number.** It is the model's calibrated guess, so
  say what produced it rather than restating it as a fact.

## Worked example: ranking candidates

Score every candidate on the same dimensions, then let code combine them. The
judgments stay raw, so the weighting is yours to change without a second call.

```python
import json, subprocess

CANDIDATES = {
    "alpha": "A plain text export, one row per record.",
    "beta": "A zipped archive with a manifest and per-table files.",
}

questions = {
    "best": {
        "type": "choice",
        "instructions": "Which candidate in `candidates` is easiest for a "
                        "non-technical person to open and read?",
        "criteria": CANDIDATES,
    },
}
for key in CANDIDATES:
    questions[f"portable__{key}"] = {
        "type": "score",
        "instructions": f"How portable is `candidates.{key}` across tools?",
        "criteria": ["Needs custom code", "Opens in common tools", "Opens anywhere"],
    }

body = {"state": {"candidates": CANDIDATES}, "model": "jev-latest",
        "questions": questions}

out = subprocess.run(
    ["lucidos", "proxy", "typesafe", "/systemone", "-X", "POST",
     "-H", "Content-Type: application/json", "--data-stdin"],
    input=json.dumps(body), capture_output=True, text=True)

reply = json.loads(out.stdout)
if "answers" not in reply:
    raise SystemExit(f"Jev refused the call: {reply.get('detail', out.stdout)}")

answers = reply["answers"]
print(answers["best"]["choice"], answers["best"]["confidence"])
for key in CANDIDATES:
    print(key, answers[f"portable__{key}"]["score"])
```

## Errors

`lucidos proxy` mirrors curl: it exits 0 on a 4xx and writes the body to
stdout. `--fail` buys a non-zero exit but discards that body, losing the one
thing that says what went wrong. Read the body instead, the way the example
above does: a refused call carries `detail` and no `answers`.

| Status | Means |
|---|---|
| `404` | No credential is configured, or the path repeated the `/v1` prefix |
| `400` or `422` | A question failed validation, and `detail` names it |
| `401` | The stored credential is wrong |
| `429` | Rate limited, so back off exponentially |
| `529` | Overloaded, so back off exponentially |
