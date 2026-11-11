---
name: Team workspace, issue tracker template
description: The issue tracker template of the team workspace setup. A signed webhook, a translation script, a keyed intake trigger, and question and outcome scripts. GitHub is the worked example.
---

<!-- domain-event: IssueTrackerDelivered: the webhook's pinned event in this template. -->
<!-- domain-event: IssueActivityReceived: the translation script in this template emits it. -->

# Issue tracker template

Read `system-knowhow/team-workspace` first. This page is one source of work for it. The worked example is GitHub issues. For another tracker, change the two marked functions in `translate.py` and the post call in the two outbound scripts. The events, the intake trigger and the factory app stay the same.

## What flows where

| Step | Piece | Does |
|---|---|---|
| 1 | Webhook `issue-tracker` | Verifies the tracker's signature and emits `IssueTrackerDelivered` |
| 2 | Script trigger `issue-tracker-translate` | Maps the author to a teammate's email, skips the bot and strangers, emits `IssueActivityReceived` |
| 3 | Intent trigger `issue-tracker-intake` | Starts or continues the root thread, keyed by repository and issue number |
| 4 | Script trigger `issue-tracker-questions` | Mirrors each gate question to the issue as a comment |
| 5 | Script trigger `issue-tracker-outcomes` | Posts the root thread's answer to the issue, short, with a link back |

A comment on the issue arrives through steps 1 to 3 again. Its key matches the open thread, so it continues the root thread. When that thread waits on a question, the comment answers it.

## 1. The bot account and the API entry

Create a bot account for the factory, never a person's account. On GitHub that is a GitHub App installed on the chosen repositories only, or a machine user with access to them alone. Give it issue read and write, contents write and pull requests write, and nothing more.

Save its token as a workspace credential named `issue-tracker-bot`, scoped to the API's base URL (`lucidos-cli.md` § `lucidos credentials`). Then add the entry the scripts call to `data/config/apis.json`:

```json
{
  "issue-tracker": {
    "base_url": "https://api.github.com",
    "auth": { "pipeline": [
      { "type": "static_credential", "kind": "bearer", "credential": "issue-tracker-bot" }
    ] }
  }
}
```

The coding agent pushes branches and opens pull requests with the same bot. Log `gh` in on the team host as the bot, never as a teammate.

## 2. The webhook

```bash
lucidos webhooks create --name issue-tracker --event-type IssueTrackerDelivered \
  --hmac '{"credential":"issue-tracker-webhook",
           "signature_header":"X-Hub-Signature-256",
           "prefix":"sha256=","template":"{body}"}' \
  --signing-secret '{"mode":"generate"}' \
  --headers '["X-GitHub-Event"]' \
  --dedupe '{"header":"X-GitHub-Delivery","window_secs":3600}'
```

Check the flags against `lucidos-cli.md` § `lucidos webhooks`, which is the authority. Paste the printed secret into the tracker's webhook form. Send only the `Issues` and `Issue comments` events, as JSON. Expose the hook socket as `remote-access.md` § "The one exception: a webhook's own socket" says.

## 3. The translation script trigger

Copy `system-knowhow/team-workspace/issue-tracker/translate.py` to `data/triggers/issue-tracker-translate/scripts/run.py`. Set `BOT_LOGIN` to the bot's login. Then:

```bash
lucidos triggers create --name "Issue tracker translate" --slug issue-tracker-translate \
  --on '[{"event_type":"IssueTrackerDelivered"}]' \
  --run '{"type":"script","path":"triggers/issue-tracker-translate/scripts/run.py"}'
```

List every teammate in `data/triggers/issue-tracker-translate/state/people.json`, as `{"their-login": "sam@example.com"}`. Use the email that is on their person in Settings. A login not in the list is a stranger, and its issues and comments start nothing. The script never trusts a public profile email, since anyone can open an issue on a public repository. So the list is who may start work from the tracker.

## 4. The intake trigger

```bash
lucidos triggers create --name "Issue tracker intake" --slug issue-tracker-intake \
  --on '[{"event_type":"IssueActivityReceived"}]' \
  --continue-key '["source","repo","number"]' \
  --actor author_email \
  --run '{"type":"intent","intent":"Work on this issue for the team, and follow the team gate policy."}'
```

- The key starts with `source`, the literal `issue-tracker` the script wrote. The outbound scripts read it to know the root thread is theirs.
- `actor` credits the teammate whose email it is. An answer from the issue sets them as driver.
- Its runs start coding agents, so ask the user to tick *Allow coding agents* on it (`triggers.md` § "Coding-agent grant"). Only the listed teammates steer those agents, since the translate script drops strangers.
- The coding agent pushes and opens pull requests under the workspace's grants, not the trigger's side-effect grant. A push those grants do not cover shows a card and waits for a human.

## 5. The outbound script triggers

Copy `system-knowhow/team-workspace/source_thread.py` to `data/scripts/team-workspace/source_thread.py` once, for every source. Copy `system-knowhow/team-workspace/issue-tracker/questions.py` to `data/triggers/issue-tracker-questions/scripts/run.py`, and `system-knowhow/team-workspace/issue-tracker/outcomes.py` to `data/triggers/issue-tracker-outcomes/scripts/run.py`. Set `LUCIDOS_URL` in both to the team workspace's address, workspace path included.

```bash
lucidos triggers create --name "Issue tracker questions" --slug issue-tracker-questions \
  --on '[{"event_type":"UserQuestionAsked"}]' \
  --run '{"type":"script","path":"triggers/issue-tracker-questions/scripts/run.py"}'
lucidos triggers create --name "Issue tracker outcomes" --slug issue-tracker-outcomes \
  --on '[{"event_type":"ResponseGenerated"}]' \
  --run '{"type":"script","path":"triggers/issue-tracker-outcomes/scripts/run.py"}'
```

Both skip any thread whose root thread the tracker did not start. The outcome script posts only the root thread's answers, never a sub-thread's. Both stay short and link back: the issue is a ramp into the workspace, never a mirror of it.

## Check it end to end

1. Open a test issue as a teammate. A thread starts, named by the issue, with no driver.
2. Ask the agent to ask a question. A comment with the question appears on the issue.
3. Reply on the issue. The question card shows answered, and the factory app names the teammate as driver.
4. Archive the thread. The next comment on that issue starts a fresh root thread.
