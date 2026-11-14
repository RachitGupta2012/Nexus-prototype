# Provider adapters

NEXUS talks to documented HTTP APIs. It does not launch agent processes, install
models, scan the network, or implement the Mission Engine.

## Protocol status

| Protocol | Send chat | Test connection | Fetch models | Notes |
| --- | --- | --- | --- | --- |
| `open_ai_chat` | Implemented | Implemented | `GET /models` | OpenAI and OpenAI-compatible gateways |
| `ollama_chat` | Implemented | Implemented | `GET /api/tags` | Native Ollama chat API, NDJSON streaming |
| `anthropic_messages` | Implemented | Implemented | `GET /models` | Native Messages API, SSE streaming, cursor pagination |
| `gemini_generate_content` | Implemented | Implemented | `GET /models` | Native Generate Content, SSE streaming, page tokens |
| `open_ai_responses` | Implemented | Implemented | `GET /models` | Native Responses API, named SSE events |

All configured protocols in this build are implemented. No fake success is
returned anywhere in the provider layer.

## Connectivity semantics

**Test connection** validates the endpoint first, then performs one model-list
request with an 8-second timeout and no redirects. It never generates tokens.

The report distinguishes:

- URL validation passed (before any request).
- Server reachable (an HTTP response was received).
- Authentication accepted or rejected (`401`/`403`), or not required.
- Model list responded and parsed.

A successful probe means the endpoint and credentials work for model listing. It
does **not** prove inference works, and the UI says so. Connectivity findings are
not stored as capability evidence or permission to execute anything.

Failures are reported without echoing response bodies, headers, or keys:

| Condition | Report |
| --- | --- |
| Invalid scheme/credentials in URL, remote HTTP, no-key remote endpoint | configuration error before the request |
| DNS, connection, or TLS failure | server unreachable |
| Timeout | no answer within 8 seconds |
| `401`/`403` | authentication rejected |
| Other HTTP status | server answered with that status; path/credentials may be wrong |
| Body not in the expected shape | server responded, model list was not in the expected format |

## Model discovery

**Fetch models** is supported for `open_ai_chat`, `open_ai_responses`,
`ollama_chat`, `anthropic_messages`, and `gemini_generate_content`.

- Base URLs are joined as-is; prefixes and trailing slashes are preserved.
- `/v1` is never duplicated. `https://host/v1` → `https://host/v1/models`;
  `https://host/gateway/v1` → `https://host/gateway/v1/models`.
- Anthropic uses cursor pagination (`limit`, `after_id`, `has_more`, `last_id`).
- Gemini uses page-token pagination (`pageSize`, `pageToken`, `nextPageToken`).
  Only models whose `supportedGenerationMethods` contains `generateContent` are
  offered.
- Both paginated listings are bounded to five pages and 2,000 models. The
  OpenAI-compatible, OpenAI Responses, and Ollama listings return a single list;
  the published OpenAI `/models` endpoint has no page cursor. That list does not
  expose which models support the Responses API, so NEXUS makes no compatibility
  claim beyond the provider accepting the request.
- Responses are bounded by the provider module's size limits.
- Discovered models are staged in the UI. Existing catalog entries and prices
  are kept; only checked, new model IDs are appended, and nothing is written to
  disk until the user saves the configuration.
- Model files are never downloaded or installed.

## OpenAI Responses adapter

Verified against OpenAI's published Responses create and streaming-event
references.

- Endpoint: `POST {base_url}/responses` (default base `https://api.openai.com/v1`).
- Authentication: `Authorization: Bearer` (existing `Bearer` auth mode). The key
  remains in the Keychain.
- Request body:
  - `model`, `stream: true`, and an explicit `store: false`. NEXUS does not
    retrieve stored responses, so provider-side retention is not requested.
  - `input` is the Responses array of `{role, content}` messages. User messages
    map to `user`, assistant messages to `assistant`; order and meaningful text
    are preserved.
  - System prompt and stored system messages map to top-level `instructions`,
    never to an input role or Chat Completions `messages`.
  - Empty text blocks are skipped safely. Consecutive same-role turns are kept
    as they are rather than merged or given invented turns.
  - `temperature` is optional (0–2). `max_output_tokens` is sent only when
    configured (1–1,000,000).
  - Chat Completions fields (`messages`, `max_tokens`, `system`) are never sent.
  - Tool definitions, assistant tool calls, and tool-result messages are
    rejected with a configuration error.
- Streaming uses named SSE events; each `data` JSON also carries `type`:
  - `response.output_text.delta` → assistant text.
  - `response.refusal.delta` → text plus a refusal outcome; `response.refusal.done`
    also marks the refusal.
  - `response.reasoning_summary_text.delta` and `response.reasoning_text.delta`
    → reasoning events, never assistant text.
  - `response.output_item.added/done`, `response.content_part.added/done`,
    `response.output_text.done`, `response.refusal.done`, reasoning done/part
    events, and unknown events are not appended as text, avoiding duplicated
    output.
  - `response.completed` → completed, unless status/refusal says otherwise.
  - `response.incomplete` → `max_output_tokens` maps to a visible token-limit
    notice; `content_filter` maps to blocked; other reasons map to incomplete.
  - `response.failed` and `error` fail with a sanitized request error.
  - A stream without a terminal event emits an incomplete outcome.
- Unsupported output: a tool-call or other non-text output item fails with
  `UnsupportedOperation` instead of being discarded and marked complete.
- Usage comes from the terminal `response.usage`: input, output, and cached
  input tokens.
- HTTP status mapping: `400` → invalid request, `401` → authentication rejected,
  `403` → permission denied, `404` → model not found, `429` → rate limited,
  other statuses → `HttpStatus`. Response bodies are never read or echoed.
- Cancellation uses the shared provider watch channel.
- Documentation:
  <https://developers.openai.com/api/reference/resources/responses/methods/create>
  and <https://developers.openai.com/api/reference/resources/responses/streaming-events>.

## Anthropic Messages adapter

Verified against Anthropic's published API and Python SDK type definitions.

- Endpoint: `POST {base_url}/messages` (default base `https://api.anthropic.com/v1`).
- Headers: `x-api-key` (existing `XApiKey` auth mode) and the required
  `anthropic-version: 2023-06-01`. No `Authorization` header is sent.
- Request body:
  - `model`, required `max_tokens`, `messages`, `stream: true`.
  - `system` is a top-level string, never a message role. Stored system messages
    are appended there instead of being dropped.
  - `max_tokens` defaults to 4,096 when the UI has no value; explicit values must
    be 1–1,000,000. The API still enforces per-model limits.
  - `temperature` is optional and must be 0–1.
  - Empty user/assistant text blocks are skipped safely; message order is kept.
  - Tool definitions, assistant tool calls, and tool-result messages are
    rejected with a clear configuration error rather than silently discarded.
    Anthropic tool execution is not implemented.
- Streaming (`text/event-stream`) parses named SSE events and trusts the JSON
  `type` field:
  - `message_start` — input and cache-read usage.
  - `content_block_start` — text/thinking block preamble.
  - `content_block_delta` — `text_delta` → text, `thinking_delta` → reasoning;
    `signature_delta`, `input_json_delta`, and citations do not become text.
  - `content_block_stop` — ignored.
  - `message_delta` — cumulative output-token usage.
  - `message_stop` — final event with usage.
  - `ping` and unknown event types are ignored.
  - `error` fails the stream without echoing its message.
  - A stream that ends without `message_stop` still emits a final event and keeps
    received text.
  - Malformed JSON fails with `InvalidResponse`.
- HTTP status mapping (no response body is read or displayed): `400`/`413`/`422`
  → invalid request, `401` → authentication rejected, `403` → permission denied,
  `404` → model not found, `429` → rate limited, other statuses → `HttpStatus`.
- Cancellation uses the shared provider watch channel.
- Model listing: `GET {base_url}/models` with `anthropic-version`, `limit=1000`,
  and `after_id` while `has_more` is true.

## Gemini Generate Content adapter

Verified against the published Gemini API reference and model-list schema
(`developers.google.com` / `ai.google.dev`; API version `v1beta`).

- Endpoints:
  - `POST {base_url}/models/{model}:streamGenerateContent?alt=sse`
  - Model listing: `GET {base_url}/models`
  - Default base URL: `https://generativelanguage.googleapis.com/v1beta`.
- Authentication: `x-goog-api-key` (existing `XGoogApiKey` mode). No
  `Authorization` header is sent.
- Request body:
  - `contents` is an ordered list. User messages become role `user`; assistant
    messages become role `model`.
  - System text is a top-level `systemInstruction.parts` list. Stored system
    messages are appended there, never sent as a `contents` role.
  - Empty text blocks are skipped safely; meaningful text and order are kept.
  - Consecutive same-role turns are preserved as-is rather than silently merged
    or given invented turns. The API may reject them, and that error is surfaced.
  - `generationConfig.temperature` is optional (0–2). `maxOutputTokens` is sent
    only when configured and must be 1–1,000,000; otherwise the model default is
    used.
  - The model is carried only in the URL path, never in the body. OpenAI- and
    Anthropic-specific fields (`model`, `stream`, `max_tokens`, top-level
    `system`) are not sent.
  - Tool definitions, assistant tool calls, and tool-result messages are
    rejected with a configuration error. Gemini tool execution is not
    implemented.
- Streaming (`text/event-stream`, `alt=sse`) reads one `GenerateContentResponse`
  JSON object per SSE `data` value:
  - Text arrives in `candidates[].content.parts[].text`.
  - Parts with `thought: true` become reasoning events, never assistant text.
  - Empty candidates and unknown part fields are ignored safely.
  - Arbitrary chunk boundaries, several SSE events per chunk, CRLF, and
    malformed JSON are handled by the shared incremental SSE decoder.
  - Usage is read from cumulative `usageMetadata`: prompt, candidate, and cached
    content token counts.
  - `promptFeedback.blockReason` and candidate finish reasons are classified
    separately from normal completion.
- Finish reasons:
  - `STOP` → completed.
  - `MAX_TOKENS` → completed with a visible "may be incomplete" notice.
  - `SAFETY`, `BLOCKLIST`, `PROHIBITED_CONTENT`, `SPII`, `IMAGE_SAFETY` →
    blocked; the partial text, if any, is stored with `Failed` status and a
    sanitized error is shown.
  - `RECITATION` → recitation-blocked with its own message.
  - `OTHER`, `LANGUAGE`, tool-call failures, unknown values, and streams with no
    finish reason → incomplete; never reported as a normal answer.
- HTTP status mapping (response bodies are never read or echoed): `400` →
  invalid request, `401` → authentication rejected, `403` → permission denied,
  `404` → model not found, `429` → rate limited, other statuses → `HttpStatus`.
  An SSE `error` object fails with a sanitized request error.
- Cancellation uses the shared provider watch channel.
- Model discovery: page size 1,000, follows `nextPageToken` for at most five
  pages/2,000 models, uses `baseModelId` (falling back to `name` without
  `models/`), and excludes models that do not declare `generateContent`.

## Ollama adapter

`ollama_chat` targets the native `POST /api/chat` endpoint, not an
OpenAI-compatible shim.

- Request body: `{model, messages, stream: true, options}` where `options`
  contains `temperature` and/or `num_predict` when configured.
- Messages map `system`, `user`, and `assistant` roles. Tool messages and
  assistant tool calls are not sent; Ollama tool support is not implemented.
- Responses are newline-delimited JSON, not SSE:
  - `{"message":{"content":"..."}}` emits text deltas.
  - `{"message":{"thinking":"..."}}` emits reasoning deltas.
  - `{"done":true,"prompt_eval_count":N,"eval_count":M}` emits the final event
    with usage.
  - A stream that ends without `done` still emits a final event and keeps the
    text received so far.
  - Malformed JSON fails the stream with a sanitized error.
  - HTTP `404` maps to "the provider does not have this model available".
- Chunk boundaries are irrelevant: multiple events per chunk and events split
  across chunks are handled by an incremental decoder.
- Cancellation uses the existing provider watch channel.
- Only the explicitly configured Ollama endpoint is contacted. NEXUS never runs
  shell commands, installs Ollama, or downloads models.

Typical endpoint: `http://127.0.0.1:11434` with authentication mode `none`.
Plain HTTP is allowed only for loopback; a remote Ollama endpoint must use HTTPS
and an allowed authentication mode.

## Architecture

- `src/provider/mod.rs` — protocols, settings validation, endpoint construction,
  error types, background `ProviderController`.
- `src/provider/http.rs` — one HTTP client path (timeouts, no redirects, auth
  headers, bounded GET and streaming helpers).
- `src/provider/openai_chat.rs` — SSE adapter.
- `src/provider/ollama_chat.rs` — NDJSON adapter.
- `src/provider/gemini_generate_content.rs` — native Generate Content SSE adapter.
- `src/provider/openai_responses.rs` — native Responses API named-event adapter.
- `src/provider/probe.rs` — connectivity probes and model discovery, run on a
  worker thread.
- `src/ui/providers.rs` — profile editor, Test connection / Fetch models, staged
  model review, Keychain key management.

Provider networking never runs inside UI painting code, and the UI never handles
raw credentials.

## Limitations

- MCP client support is read-only: explicit connection, protocol negotiation, capability display, and bounded tool discovery. Tool execution, resource reads, prompt retrieval, ACP integration, and the mission runner are absent.
- No streaming cancellation control in the chat UI beyond dropping the
  controller.
- Model listing does not verify that a model can generate output.
- Automated tests use loopback mock servers and fixed payloads; live provider
  requests are manual and billable. Gemini was not exercised against the live API.
- Gemini may answer an invalid key with HTTP `400` rather than `401`; NEXUS
  reports that as an invalid request because it never reads error bodies.

## Manual Ollama verification

With Ollama running locally and a model already installed (NEXUS does not install
either):

```sh
NEXUS_OLLAMA_MODEL=llama3.2 \
  cargo test --workspace live_ollama -- --ignored --nocapture
```

Or use the Providers screen: Test connection, Fetch models, select a model, save,
then send a message. Unavailable-model handling can be checked by selecting a
model that is not installed; the failure is reported as an HTTP 404/model-not-found
error.
