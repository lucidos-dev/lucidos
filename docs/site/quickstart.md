# Quickstart

Get Lucidos running on a clean macOS or Linux machine, then point it at an LLM
provider.

On macOS the fastest route is the signed, notarized desktop app: grab the
`.dmg` from the [latest release](https://github.com/lucidos-dev/lucidos/releases/latest)
and drag it to Applications. The one-line installer below is the headless path
(browser UI plus an always-on service), and the only path on Linux. Both are
covered in full below.

{%
   include-markdown "../../README.md"
   start="<!--quickstart-start-->"
   end="<!--quickstart-end-->"
%}

## Working on Lucidos itself

Everything above is about *running* Lucidos. Running it from a source checkout is
a different activity, and the reason to do it is not that you get a runtime out
of it: it is that Lucidos can then work on its own code, proposing each change as
a diff you review and Apply. See **[Develop Lucidos](develop.md)** for that loop,
the dev setup it needs, and how to contribute.

## Next steps

- **[Concepts](concepts.md)** — the building blocks you'll work with.
- **[Build your first app](tutorials/build-an-app.md)** — describe an app in chat and watch it appear.
- **[Automate with a trigger](tutorials/automate-with-a-trigger.md)** — make things happen on a schedule or in response to events.
