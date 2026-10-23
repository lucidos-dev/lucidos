You are reviewing a bug fix written by a coding agent. Judge whether it fixes
the root cause, or papers over the symptom.

You get four things:

1. The bug report the agent received.
2. The fix the maintainers actually shipped, as a diff. Treat it as one good
   answer, not the only one. A different change that removes the cause is
   just as good.
3. The agent's diff against the same starting code.
4. The agent's own summary of what it did.

## Scoring

| Score | Meaning |
|---|---|
| 5 | Removes the root cause, as completely as the shipped fix or better. |
| 4 | Removes the root cause, with a minor gap or a rough edge. |
| 3 | Partial. Fixes part of the cause, or only the reported inputs, by a reasonable change. |
| 2 | Papers over. Suppresses the symptom downstream, special-cases the reported input, or changes only tests or docs. |
| 1 | No meaningful fix, wrong, or it breaks other behaviour. |

"Papered over" means the change hides the symptom while the cause stays in
place. Judge the code, not the summary. A confident summary over a weak diff
is still a weak diff.

## Output

Reply with one JSON object and nothing else:

```json
{
  "score": 1,
  "papered_over": false,
  "root_cause_identified": true,
  "reasoning": "two to four sentences"
}
```

## The bug report

{{BUG}}

## The shipped fix

```diff
{{REFERENCE}}
```

## The agent's diff

```diff
{{AGENT_DIFF}}
```

## The agent's summary

{{AGENT_SUMMARY}}
