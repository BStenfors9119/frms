# frms Telemetry — Backend Endpoint Spec

What the backend must implement to receive frms usage telemetry. The client is
`src/telemetry.rs`; this document is the contract between it and the server.

> Status: the client ships and POSTs to the URL below. The route may not exist
> yet — the client is **fire-and-forget and ignores the response**, so a missing
> route (404) is harmless, but no data is stored until the route is live.

---

## Endpoint

```
POST https://api.thesportsremote.com/api/telemetry
Content-Type: application/json
```

- **Method:** `POST` only.
- **URL/path:** `/api/telemetry`. The client default is
  `https://api.thesportsremote.com/api/telemetry`; it can be overridden per-install
  with the `FRMS_TELEMETRY_URL` env var ("until we move it" — no client rebuild).
- **TLS:** HTTPS required (the client uses `https`).
- **Auth:** **none today.** The client sends no API key / token / custom headers
  (just `Content-Type`). See [Security & abuse](#security--abuse).
- **Body:** a single JSON object (one event per request). Not batched, not
  newline-delimited.

## Response

The client does not read or branch on the response — any status is accepted and
ignored, with a 5-second timeout. Recommended:

- Return **`204 No Content`** (or `202 Accepted`) as fast as possible.
- Do **not** require the client to read a body.
- Do **not** rely on the client honoring `4xx/5xx`, redirects, or retries — there
  are none. If you reject input, just drop it server-side.

## Request body

Every event is the same envelope with an event-specific `props` object:

```json
{
  "event": "launch",
  "id": "9f3c1a2b4d5e6f708192a3b4c5d6e7f8",
  "ts": 1751155200,
  "app": "frms",
  "app_version": "0.1.2",
  "os": "linux",
  "arch": "x86_64",
  "props": {}
}
```

### Envelope fields

| Field         | Type    | Notes |
|---------------|---------|-------|
| `event`       | string  | Event name — one of the table below. |
| `id`          | string  | **Anonymous install id** — 32 lowercase hex chars (16 random bytes), stable per machine, stored in `~/.frms/telemetry_id`. Pseudonymous; **not** a user/person and not reversible. |
| `ts`          | integer | Unix epoch **seconds** (client clock; may be skewed — prefer server receive-time for ordering). |
| `app`         | string  | Always `"frms"`. |
| `app_version` | string  | Semver of the build, e.g. `"0.1.2"`. |
| `os`          | string  | Rust `std::env::consts::OS` — `"linux"`, `"macos"`, etc. |
| `arch`        | string  | Rust `std::env::consts::ARCH` — `"x86_64"`, `"aarch64"`, etc. |
| `props`       | object  | Event-specific; may be empty `{}`. |

### Events and their `props`

| `event`           | `props`                              | Meaning |
|-------------------|--------------------------------------|---------|
| `launch`          | `{}`                                 | App started. |
| `agent_created`   | `{ "kind": "build"\|"research"\|"chat" }` | An agent pane was created. |
| `session_created` | `{ "kind": "terminal"\|"browser" }`  | A session was created. |
| `chat_completed`  | `{ "model": "<model id>" }`          | A Research/Chat reply finished OK (e.g. `claude-opus-4-8`, `claude-sonnet-4-6`). |
| `error`           | `{ "kind": "chat", "detail": "<text>" }` | A non-fatal error surfaced. `detail` is scrubbed + ≤500 chars. |
| `panic`           | `{ "detail": "<text>" }`             | A caught panic (location + message). `detail` is scrubbed + ≤500 chars. |

> Treat `event` / `props` as **open**: tolerate unknown event names and extra
> `props` keys (forward-compatible as the client adds events). Don't 500 on them.

### Examples

```json
{ "event": "agent_created", "id": "9f3c…e7f8", "ts": 1751155210, "app": "frms",
  "app_version": "0.1.2", "os": "linux", "arch": "x86_64",
  "props": { "kind": "research" } }
```
```json
{ "event": "chat_completed", "id": "9f3c…e7f8", "ts": 1751155290, "app": "frms",
  "app_version": "0.1.2", "os": "linux", "arch": "x86_64",
  "props": { "model": "claude-sonnet-4-6" } }
```
```json
{ "event": "error", "id": "9f3c…e7f8", "ts": 1751155300, "app": "frms",
  "app_version": "0.1.2", "os": "linux", "arch": "x86_64",
  "props": { "kind": "chat", "detail": "claude request failed" } }
```

## Privacy guarantees (what you will and won't receive)

The client is built so these never leave the machine — the backend should
**neither expect nor store** anything beyond what's documented:

- **No PII / no SOC 2-sensitive data.** No usernames, hostnames, IPs (beyond the
  TCP source the server sees), emails, file names, paths, project names, prompt
  or chat content, DB rows, or keys.
- `id` is random and anonymous — do not try to join it to a person.
- `error`/`panic` `detail` is already scrubbed client-side (home dir → `~`,
  secret-shaped tokens → `[redacted]`, truncated to 500 chars). Treat it as a
  coarse hint, not structured data. **Recommend: don't index/search it; consider
  re-scrubbing or dropping it on ingest** as defense-in-depth.
- The source IP is visible to the server at the TCP layer. If even coarse geo/IP
  is undesirable, avoid logging it or truncate/hash it at ingest.

## Suggested storage

Append-only; events are **not** deduplicated (no per-event id is sent), so don't
assume uniqueness. A minimal table:

```sql
CREATE TABLE telemetry_event (
  received_at  timestamptz NOT NULL DEFAULT now(),  -- server receive time (trust this)
  client_ts    bigint,                              -- envelope.ts (advisory)
  install_id   text        NOT NULL,                -- envelope.id
  event        text        NOT NULL,
  app_version  text,
  os           text,
  arch         text,
  props        jsonb        NOT NULL DEFAULT '{}'
);
CREATE INDEX ON telemetry_event (event, received_at);
CREATE INDEX ON telemetry_event (install_id);
```

This answers the questions the telemetry is for: active installs
(`count(distinct install_id)` over a window), version/OS/arch spread, feature
usage (`agent_created`/`session_created`/`chat_completed` by `props`), and
error/crash rates (`error`/`panic` counts by `app_version`).

## Validation (be lenient)

- Require `event` (string) and `id` (string). Everything else is best-effort.
- Cap request body size (e.g. reject > 8 KB — events are small).
- Ignore/sanitize unknown fields rather than rejecting.
- Be liberal: a malformed event should be dropped silently, not error the client
  (which ignores errors anyway).

## Security & abuse

The endpoint is **unauthenticated and public**, so it can be spammed/spoofed:

- Add **rate limiting** per source IP and a small body-size cap.
- It's fine for anyone to POST fake events — treat the data as low-trust signal,
  not ground truth. Don't drive billing/security decisions off it.
- If you later want lightweight authenticity, add a shared ingest token the
  client sends as a header — **note this requires a coordinated client change**
  (`src/telemetry.rs` would add the header); it is **not** sent today.
- No CORS needed (the client is `curl`, not a browser).

## Operational notes

- **Volume:** one short-lived POST per event, serialized by a single client-side
  thread, 5 s timeout. Low and bursty (a launch + a few usage events per
  session). The client drops events if its local queue backs up, so the server
  is never a bottleneck for the app.
- **Opt-out:** users can disable telemetry (Profile tab) or set
  `DO_NOT_TRACK=1` / `FRMS_NO_TELEMETRY=1`; those installs send nothing — expect
  under-counting, not full coverage.
