# 0423: The proxy asks OpenAI speech for an SSE stream, so a TTS call records its cost

- **Status**: Accepted
- **Date**: 2026-10-10

## Context

An agent made six text-to-speech calls with `lucidos proxy openai
/audio/speech`. None left a `ContextCaptured`, so the Token Cost app showed no
spend for them. ADR 0242 says every model call records what it cost.

The proxy records what a reply's usage block reports (`api::proxy_cost`).
OpenAI's speech endpoint sends one only on a server-sent event stream
(`stream_format: "sse"`), in its closing `speech.audio.done` frame. Its default
reply is bare audio with no usage anywhere: not in the body, not in a header.

A live probe sent "Hi." in every `response_format`, plain and as a stream. The
stream's base64 deltas decode to:

| Format | Stream decodes to |
|---|---|
| `mp3` (default) | valid MP3 frames |
| `aac` | valid ADTS AAC |
| `pcm` | the same raw PCM |
| `wav` | raw PCM, no `RIFF` header |
| `flac` | raw PCM, no `fLaC` header |
| `opus` | Ogg pages with no `OpusHead`, which do not decode |

## Decision

**No OpenAI speech call through the proxy goes unrecorded.** By what the
request asks for:

| Request | What the proxy does |
|---|---|
| `mp3`, `aac` or `pcm` | asks for the stream, records its usage, returns the decoded audio |
| `wav` | asks for `pcm` over the stream, records it, adds a 44-byte WAV header |
| `flac` or `opus` | refuses with a 400 naming the formats that record |
| `tts-1` or `tts-1-hd` | forwards as is, then records the input's character count |
| the stream, asked by the caller | forwards as is; the stream already records |

Any other host or path is left alone, byte for byte.

## Rationale

**The provider's count where it gives one.** ADR 0242 records only what a
provider reported. A speech call's price is mostly audio output tokens, and
only the stream names them. A count derived from bytes or seconds would be a
guess that drifts with every model.

**`tts-1` is billed by characters, and the request states them.** OpenAI
prices it per 1M input characters and reports nothing back. The count is the
billing basis itself, not an estimate. Its price card says per 1M characters.

**The caller keeps the reply it asked for.** `forward_request` already buffers
every reply whole, so asking for a stream loses the caller no streaming. An
error reply, or a reply that is not a stream, passes through untouched. A
stream that ends before `speech.audio.done` is a 502, not partial audio.

**A WAV header, but no encoder.** OpenAI's own `wav` reply is PCM, 24 kHz,
mono, 16-bit, behind a fixed header, so adding that header loses nothing.
`flac` and `opus` would need an encoder in the engine, `opus` a native C
library. Refusing them costs a caller one retry in another format, where
serving them would be spend nobody sees.

## Consequences

- Every OpenAI speech call through the proxy now shows in Token Cost, once
  `pricing.json` has a card for its model.
- A caller asking for `flac` or `opus` gets a 400 and must pick another format,
  or ask for the stream itself.
- The proxy now rewrites and refuses some requests. `api::proxy_cost` names
  the exception in its header, so a reader of the forward-untouched contract
  finds it.
- OpenAI may fix the container on a streamed `flac` or `opus`. Then a fresh
  probe moves it from refused to streamed in `ask_for_usage`.

## Alternatives considered

**Tell agents to ask for the stream themselves.** No engine change, and the
existing parse already records it. Rejected: every caller would have to decode
base64 frames to get a file, and any caller that forgot would be silent spend
again. That is the failure ADR 0242 exists to end.

**Estimate `flac` and `opus` from the audio's length.** Would serve every
format. Rejected for recording a number the provider never reported, which
ADR 0242 rules out.

**Encode `flac` and `opus` in the proxy.** A pure-Rust `flac` encoder exists,
but `opus` needs libopus in every build. Rejected as a new dependency for two
formats a caller can trade for `wav` or `aac`.

**An engine-native speech tool through the model call service.** The thorough
product answer. Rejected for this change as a new surface. It does not replace this one: apps and scripts reach
OpenAI through the proxy either way.
