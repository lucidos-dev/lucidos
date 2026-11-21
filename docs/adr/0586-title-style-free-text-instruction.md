# 0586: Title style is one free-text instruction per workspace, appended to the title system prompt

- **Status**: Accepted
- **Date**: 2026-10-26

## Context

A tester wants short lower-kebab-case thread names and renamed every thread by hand. Automatic titles are one fixed format: a very short title of 3-6 words from a hardcoded system prompt.

## Decision

A workspace has one optional `title_style` preference: a free-text instruction of at most 500 characters. When set, the title call appends it to the system prompt, after the standard instruction, and says it overrides the length and wording rules. Unset or blank leaves the prompt byte-identical to today's. Settings edits it, and Reset deletes the row.

## Rationale

The title model already follows plain-language instructions, so the user states the style in their own words and the engine needs no vocabulary of styles. The style goes in the system prompt, never the user message. The message stays pure conversation text, and the model is not tempted to echo an instruction. Output validation is unchanged, and a kebab-case title is one word to it, so it passes. `title_call` reads the preference, so every site gets the style and a future site cannot forget it.

## Consequences

- A new value kind, `PrefValue::Instruction { max_chars }`: non-empty text with a length bound. The generated catalog carries `maxChars`, so the Settings counter never holds a literal.
- The style shapes only titles written from then on. Existing threads, caller-named threads and image placeholder titles keep their names.
- The change summary inherits the title model but not the style.
- The agent can set it (`PrefAccess::Agent`), so "name my threads in kebab case" works from the prompt.

## Alternatives considered

- **A preset enum of styles** (sentence, kebab, short): closed, needs a release for each new wish, and the tester's wish is one the presets would miss.
- **Deterministic post-processing** such as slugifying the title: it fixes the case but not word count or tone. It also mangles a ticket key the prompt asks the model to keep verbatim.
- **A per-device scope**: titles belong to the thread, which every device sees. A per-device style would name the same thread differently depending on who triggered the title.
