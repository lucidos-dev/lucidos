# 0747: The turn-end judge reviews trigger requests to enter Review

- **Status**: Accepted
- **Date**: 2026-11-20
- **Amends**: [0472](0472-trigger-runs-decide-their-read.md)

Record: `docs/plans/2026-11-20-routine-trigger-runs-stay-unread-free.md`.

## Context

Routine trigger replies reached Review when their agents requested a read.
An explicit yes bypassed the turn-end judge. The judge also treated reports
requested on every run as worth reading, even when the report was unchanged.

## Decision

For a trigger run and its automatic wakes, an explicit no records the decision
immediately. An explicit yes leaves the decision to the turn-end judge and the
tool result tells the agent so. Routine reports with no new finding or action
count as no. A delivered notification still covers the reply unless the reply
reports a failure the notification omitted.

## Rationale

A `TriggerStarted` opener identifies the run. A child report or an engine
event-wait opener continues it when the latest earlier initiating input on
the same thread is `TriggerStarted`. The judge sees what opened the turn
and a note that it continues the run. A later user or agent message that
opens a turn can record a read request immediately, as can its wakes.
Keeping the agent's no avoids a judge call for routine runs. The judge can use
the completed reply and delivered notifications when reviewing a yes.

On the 78 public cases, Gemini 3.8 Flash at low effort missed none of the
27 must-reads with either the baseline or final prompt. The baseline passed
two of the 51 noes and the final prompt passed none. An intermediate prompt
missed a sharp spend increase, so the final prompt names material changes in
scheduled reports as findings. The private set was unavailable for this run.

## Consequences

This applies to trigger runs across workspaces, models and install shapes.
A judge error or missing answer still yields yes. A reachable judge can
misclassify a genuine finding, so the public evaluation tracks missed
must-reads as well as routine replies that enter Review. Triggers configured
with `go_to_review` still land in Current.

## Alternatives considered

Prompt guidance alone leaves an agent's yes in control, which allowed the
routine inbox reports to reach Review. Judging every run would also review
the agent's no and add a call to routine runs that already decide correctly.
