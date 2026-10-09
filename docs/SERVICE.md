# Comrade Service API (server contract)

The desktop app requires the authenticated Comrade gateway for chat and
memory embeddings. Set its URL in **Settings → Service backend** or
`COMRADE_SERVER_URL`. Server voice also uses the gateway; local voice remains
available. An account is required in both cases. See [accounts](ACCOUNTS.md).

## Modes

| Setting | Effect |
|---|---|
| `base_url` | Brain (chat) + memory embeddings via server. Desktop always enables the gateway. Changing its URL requires signing in again. |
| `[voice] backend = server` (default) | STT + TTS via server (energy VAD on-device, **no model download**). Applies on next voice session. |
| `[voice] backend = local` | On-device sherpa-onnx STT/VAD/TTS (one-time ~185 MB download), even when the brain uses the server. |

## Endpoints (OpenAI-compatible)

Base URL = server URL without trailing slash, e.g. `http://localhost:8000`.

```text
GET  {base}/healthz  liveness
GET  {base}/readyz   200 when required keys present
GET  {base}/v1/models  routing table, no secrets (shown by Settings → Test)
POST {base}/v1/chat/completions
POST {base}/v1/embeddings
POST {base}/v1/audio/transcriptions
POST {base}/v1/audio/speech
```

Auth: `Authorization: Bearer <Supabase user access token>` on every `/v1/*`
model request. The native account manager refreshes tokens before use.
Shared server API keys do not grant access. `/healthz` and `/readyz` are public.
`GET /v1/auth/me` reports the verified account and unlimited testing mode.

### Chat — `POST /v1/chat/completions`

Request (tools are optional; function names arrive as `namespace_tool`
— DeepSeek-strict form — decode `_` back to `.` before dispatch):

```json
{
  "model": "comrade-default",
  "messages": [
    {"role": "system", "content": "..."},
    {"role": "user", "content": "hello"},
    {"role": "assistant", "content": "", "tool_calls": [
      {"id": "1", "type": "function",
       "function": {"name": "filesystem_list", "arguments": "{\"path\": \"/tmp\"}"}}
    ]},
    {"role": "tool", "tool_call_id": "1", "content": "{\"success\": true, ...}"}
  ],
  "max_tokens": 2048,
  "temperature": 0.3,
  "stream": false,
  "tools": [
    {"type": "function", "function": {
      "name": "filesystem_list", "description": "...",
      "parameters": {"type": "object", "properties": {}}}}
  ]
}
```

Non-streaming response (OpenAI shape):

```json
{"choices": [{"message": {"content": "hi", "tool_calls": []}}]}
```

The `model` id is always sent exactly as picked — one of
`nvidia/nemotron-3.5-lightning`, `deepseek-flash`, `openai/gpt-oss-120b`,
`google/gemma-4-31b-it` (Settings dropdown; anything else falls back to
`deepseek-flash` so a stale conf can never send an unknown id).

Streaming (`"stream": true`): server-sent events
`data: {"choices": [{"delta": {"content": "hi"}}]}` … `data: [DONE]`.
Tool-call deltas accumulate by `id` like OpenAI. If streaming fails, the
app retries the same payload non-streaming.

### Embeddings — `POST /v1/embeddings`

```json
// request
{"model": "comrade-embed", "input": ["text one", "text two"]}
// response
{"data": [{"index": 0, "embedding": [0.1, ...]}, {"index": 1, "embedding": [...]}]}
```

Vectors must all have the same length, matching `embedding_dim`
(Settings → Service, default 1536). Changing dim needs a fresh
`comrade-memory.db` (vectors are fixed-width).

### STT — `POST /v1/audio/transcriptions`

OpenAI-style multipart upload: `file` = wav bytes (16 kHz mono, filename
`audio.wav`), `model` = configured STT model, `language` when set.

```json
// response
{"text": "open my project"}
```

Uploads one VAD-endpointed utterance (up to `max_utterance_ms`, 30 s
default). Empty `text` = "nothing heard", the turn keeps listening.

### TTS — `POST /v1/audio/speech`

```json
// request
{"model": "comrade-tts", "input": "One sentence.", "voice": "default", "speed": 1.0, "response_format": "wav"}
// response: raw audio bytes (wav requested so no extra codec is needed)
```

Called once per reply sentence, so speech starts before the LLM
finishes. A JSON `{audio_base64, sample_rate}` envelope is also accepted
as a fallback.

## Config reference

`.env` / environment (win over Settings):

```bash
COMRADE_SERVER_URL=https://comradeserver.vercel.app
COMRADE_SERVER_LLM_MODEL=comrade-default
COMRADE_SERVER_EMBEDDING_MODEL=comrade-embed
COMRADE_SERVER_EMBEDDING_DIM=1536
COMRADE_SERVER_STT_MODEL=comrade-stt
COMRADE_SERVER_TTS_VOICE=default
COMRADE_VOICE_BACKEND=local   # local | server
```

`comrade.conf` mirrors: `[server]` (`base_url,
llm_model, embedding_model, embedding_dim, stt_model, tts_voice`) and
`[voice] backend`.
