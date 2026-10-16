When a Claude reply is cut off by the output token limit partway through a
tool call, Lucidos normally fails the turn at once. The error says the model
hit its output limit and suggests smaller steps.

One of our MCP tools is named `502`, and with it the turn does not fail fast.
The engine retries the identical request several times, and every retry is
cut off the same way. Tools named `503` and `529` do the same. It also
happens when the model stops with `tool_use` and the tool arguments are
malformed JSON. Tools with ordinary names behave correctly.
