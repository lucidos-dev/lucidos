I picked the bare Claude Opus 5.5 model (`claude-opus-5-5`, no `[1m]`
suffix). Long chats then drop their earliest history once the conversation
reaches about 200k tokens.

Opus 5.5 has a 1M-token context window by default, with no beta header, so
history should survive about five times longer. Bare Opus 5
(`claude-opus-5@default`), Fable 5 (`claude-fable-5`) and Fable 5.1
(`claude-fable-5-1`) are 1M by default too, and show the same early
trimming.

Sonnet 5 and the Opus 4.x models really are 200k by default, and they behave
correctly. Picking the `[1m]` variant of any model also works.
