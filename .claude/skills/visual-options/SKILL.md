---
name: visual-options
description: Use BEFORE the first edit of a look-and-feel change to the Lucidos UI ("doesn't look great", "sleeken", "restyle", "misplaced", "too big", spacing, colour, layout). Show rendered options for approval first. A simple choice goes as pictures on the question card, a complex one as a live preview. Covers rendering in the real app and theme.
---

# Visual options before visual edits

The user wants to pick a look, not review one. A visual change starts with
rendered options they approve. It never starts with an edit.

## When it applies

- **Applies**: any change to how the UI looks where more than one answer is
  reasonable. That covers restyling, spacing, colour, type, layout, and a new
  visual element.
- **Skip the options** when the user gave an exact spec ("make it 2px", "use
  `--accent`"). Skip them too when the fix has one correct answer, like a
  clipped label. Say you skipped them and why. The approval gate under
  "Approval before verification" still applies.

## Pick the mode

| Mode | Use it for | Shape |
|---|---|---|
| **Pictures on the card** | A static choice: one element, one state | 2 to 4 options on the question tool. Each option carries its own picture in `preview`. |
| **Live preview** | Motion, hover or focus states, several states, responsive behaviour, or more than one screen | One HTML page with a switcher for the variants. Link it in your message and ask on the card. |

**If unsure which, ask** with the question tool before you render anything.

## Render in context, with the real CSS

A mockup on a blank page hides the background, the neighbours and the edges it
has to line up with. Render inside the running app, in the workspace's theme.

1. Read the dev workspace's URL from `<workspace>/.lucidos/ports`.
2. Pick a thread with content: `psql -Atc "select thread_id from thread_summaries where has_response order by last_activity desc limit 5"`.
3. Write each variant as a CSS file plus an HTML fragment. Use the component's
   real class names and the theme tokens, never hardcoded colours.
4. Render it with the helper beside this file. It is read-only: it injects the
   fragment into the page and saves nothing to the workspace.

   ```sh
   node .claude/skills/visual-options/render-in-app.cjs \
     --url http://localhost:<port> --hash thread=<uuid> \
     --css variant-a.css --html variant-a.html --out /tmp/<slug>/a.png
   ```

   `--into` and `--capture` take selectors for a surface outside the thread
   feed. `--hover` shows a hover state.
5. **The running build still carries the old rules.** Injected CSS adds to
   them, so cancel any old declaration a variant drops. A stale `min-width`
   once made a correct X look misplaced by 22px.
6. `Read` every picture before you show it. Fix what looks wrong first.

## Show it

- Store each picture with `lucidos data write artifacts/<slug>/option-a.png --from <png>`.
  Paste the `![…](artifacts/…)` line it prints into that option's `preview`.
  A picture you only `Read` or store is invisible to the user.
- One picture per option, of that option only. The option's `description` says
  in one line what differs.
- A live preview goes to `artifacts/<slug>/preview.html`. It must be
  self-contained: inline the tokens and rules it needs. Link it as
  `[Open the live preview](artifacts/<slug>/preview.html)`.

## After the answer

- **Feedback instead of a pick** ("highlight the question more") means render
  again. Do not start editing on a half-answer.
- **A pick** is the approval, and an exact spec means build that. Either way, build exactly that variant. Then
  re-render the real markup with your new CSS and measure the edges it must
  share with its neighbours (`getBoundingClientRect`). Do not trust your eyes
  alone.

## Approval before verification

A visual change is approved one of two ways, and one is enough:

- **The user picks a rendered option.** The pick is the approval. Build that
  variant, match it to the picture, then verify. Do not ask a second time.
- **The user approves the look you built.** This applies when there were no
  options (an exact spec, a one-answer fix), or when the build had to depart
  from the picked picture.

**Run no test, browser spec or `/harden` until one of those holds.** The user
asked for this gate. A look often takes several rounds, and verifying each one
spends a full test run on work about to change.

For the second way:

1. Build the look and make sure it compiles (`tsc`, `vite build`).
2. Commit it as a checkpoint.
3. Ask on the question tool: `Approve` or `Request changes`. Once the user
   iterates on the built look, they preview the branch themselves, so send no
   rendered options or pictures.
4. On feedback, change the look and go back to step 1.

Once approved either way, update the tests the change breaks, then run the
suites and `/harden` once over the whole batch.

This card is a decision before verification, so it is not the "does this look
good?" question the general rules forbid after finished work.
