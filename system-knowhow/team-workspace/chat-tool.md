---
name: Team workspace, chat tool template
description: The chat tool template of the team workspace setup. A polling translation script, a keyed intake trigger, and question and outcome scripts that reply in the chat thread. Slack is the worked example.
---

<!-- domain-event: ChatMessageReceived: the translation script in this template emits it. -->

# Chat tool template

Read `system-knowhow/team-workspace` first. This page is one source of work for it. The worked example is Slack. A message that mentions the factory's bot starts a work item. Replies in that chat thread continue it, and answer its questions.

## Why it polls

Polling needs no public address. The template reads the Web API once a minute through `lucidos proxy`, so it works on a laptop with nothing exposed. The cost is up to a minute of delay.

To take Slack's Events API instead, publish the hook socket and use the issue tracker template's shape: a webhook, then a translation script on its event. A hook verifying `X-Slack-Signature` echoes Slack's `url_verification` challenge, so Slack accepts the Request URL. `system-knowhow/lucidos-cli` § `lucidos webhooks` shows the setup order.

| Step | Piece | Does |
|---|---|---|
| 1 | Script trigger `chat-translate`, every minute | Reads new messages, resolves user ids to emails, emits `ChatMessageReceived` |
| 2 | Intent trigger `chat-intake` | Starts or continues the work item, keyed by channel and chat thread |
| 3 | Script trigger `chat-questions` | Posts each gate question into the chat thread |
| 4 | Script trigger `chat-outcomes` | Posts the root thread's answer into the chat thread, short, with a link back |

## 1. The bot and the API entry

Create a Slack app for the factory with a bot user, installed only in the channels it serves. Give it `channels:history`, `chat:write`, `users:read` and `users:read.email`. Add `groups:history` for a private channel. Invite the bot to each channel.

Save the bot token as a workspace credential named `chat-bot`, scoped to `https://slack.com/api`. Then add the entry to `data/config/apis.json`:

```json
{
  "chat": {
    "base_url": "https://slack.com/api",
    "auth": { "pipeline": [
      { "type": "static_credential", "kind": "bearer", "credential": "chat-bot" }
    ] }
  }
}
```

## 2. The translation script trigger

Copy `system-knowhow/team-workspace/chat-tool/translate.py` to `data/triggers/chat-translate/scripts/run.py`. Set `CHANNELS` and `BOT_USER`. Then:

```bash
lucidos triggers create --name "Chat translate" --slug chat-translate \
  --cron-expressions '["0 * * * * *"]' \
  --run '{"type":"script","path":"triggers/chat-translate/scripts/run.py"}'
```

- The first run only starts the clock. Older messages start nothing.
- It follows each chat thread it started for 14 days.
- List the teammates in `data/triggers/chat-translate/state/people.json`, as `["sam@example.com"]`, with the email on each person in Settings. Anyone else in the channel, a guest included, is a stranger and starts or steers nothing. A Slack email outside the list reads as a stranger too.

## 3. The intake trigger

```bash
lucidos triggers create --name "Chat intake" --slug chat-intake \
  --on '[{"event_type":"ChatMessageReceived"}]' \
  --continue-key '["source","channel","thread_ts"]' \
  --actor author_email \
  --run '{"type":"intent","intent":"Work on this request from the team chat, and follow the team gate policy."}'
```

The key starts with `source`, the literal `chat` the script wrote. Ask the user to tick the side-effect grant the work needs (`triggers.md` § "Side-effect grant").

## 4. The outbound script triggers

Copy `system-knowhow/team-workspace/source_thread.py` to `data/scripts/team-workspace/source_thread.py`, if the issue tracker template has not already. Copy `system-knowhow/team-workspace/chat-tool/questions.py` to `data/triggers/chat-questions/scripts/run.py`, and `system-knowhow/team-workspace/chat-tool/outcomes.py` to `data/triggers/chat-outcomes/scripts/run.py`. Set `LUCIDOS_URL` in both.

```bash
lucidos triggers create --name "Chat questions" --slug chat-questions \
  --on '[{"event_type":"UserQuestionAsked"}]' \
  --run '{"type":"script","path":"triggers/chat-questions/scripts/run.py"}'
lucidos triggers create --name "Chat outcomes" --slug chat-outcomes \
  --on '[{"event_type":"ResponseGenerated"}]' \
  --run '{"type":"script","path":"triggers/chat-outcomes/scripts/run.py"}'
```

The bot's own posts come back in the history, and the translation script skips them. So a mirrored question never answers itself.

## Check it end to end

1. Mention the bot in a channel it serves. Within a minute a thread starts.
2. Ask the agent to ask a question. The question appears in the chat thread.
3. Reply in the chat thread. The question card shows answered, and you are the driver.
4. Archive the thread. The next reply in that chat thread starts a fresh work item.
