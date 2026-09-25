# Quickstart

Install Lucidos on a clean macOS or Linux machine, then connect it to an LLM
provider.

On macOS, the fastest route is the signed, notarized desktop app. Download the
`.dmg` from the [latest release](https://github.com/lucidos-dev/lucidos/releases/latest)
and drag it to Applications. The one-line installer below is the headless path
(browser UI plus an always-on service), and the only path on Linux. This page
covers both.

{%
   include-markdown "../../README.md"
   start="<!--quickstart-start-->"
   end="<!--quickstart-end-->"
%}

## Working on Lucidos itself

Run Lucidos from a source checkout and it can work on its own code. It proposes
each change as a diff that you review and Apply. See
**[Develop Lucidos](develop.md)** for that loop, its dev setup, and how to
contribute.

## Next steps

- **[Concepts](concepts.md)**: the building blocks you'll work with.
- **[Build your first app](tutorials/build-an-app.md)**: describe an app in chat and watch it appear.
- **[Automate with a trigger](tutorials/automate-with-a-trigger.md)**: run work on a schedule or in response to events.
