# Stalky Backend

A versioned, authenticated Rust (Axum) backend for Stalky and the Aura Android
compatibility client. It is a member of the Stalky workspace (`package name:
stalky-backend`) and exposes:

- `GET /health/live` and `GET /health/ready` — liveness/readiness probes.
- `GET /v1/me` and `GET/PATCH /v1/profile` — verified identity and tenant profile.
- `/v1/memories`, `/v1/todos`, and `/v1/mini-apps/{id}/records` — durable,
  user-scoped application state.
- `/v1/assistant/runs` — durable run admission, polling, cancellation,
  idempotency, and ordered events.
- `POST /v1/assistant/chat` — Rust-native Gemini, OpenAI Responses, and
  OpenRouter provider adapters with server-only encrypted credential handling.

Equivalent resource paths are mounted under `/api/` for the existing Android
client. Aura's former password/Google/refresh-token handlers are compatibility
routes that return a stable `501` until the client completes its Supabase Auth
migration. Mini-app generation, model discovery, and transcription remain
explicit `501` routes. Durable assistant runs are executed by the separate
`agent-worker` binary; chat calls providers directly through the Rust adapter
registry.

The OpenAPI 3.1 contract lives in [`openapi.yaml`](openapi.yaml). The Supabase
migrations are in [`migrations/0001_profiles.sql`](migrations/0001_profiles.sql)
and [`migrations/0002_application_state.sql`](migrations/0002_application_state.sql)
plus [`migrations/0003_agent_leases.sql`](migrations/0003_agent_leases.sql).

## Local development

Prerequisites: Rust stable (the workspace pins edition 2024).

```sh
# from the repository root
cargo run -p stalky-backend
```

By default the backend binds to `127.0.0.1:8080` and requires
`SUPABASE_URL`. Provide it inline or via a shell-sourced env file:

```sh
export SUPABASE_URL="https://<project-ref>.supabase.co"
export BIND_ADDRESS="127.0.0.1:8080"
export DATABASE_URL="postgresql://<user>:<password>@<host>:5432/postgres"
export STALKY_PROVIDER_CREDENTIAL_KEY="<64 hex characters>"
export RUST_LOG="info"
cargo run -p stalky-backend

# in a separate process, after migrations are applied
cargo run -p stalky-backend --bin agent-worker
```

`DATABASE_URL` selects the Postgres/Supabase store. If omitted, local
development uses process-local in-memory persistence and logs a warning; a
production deployment should always configure the database URL.

Quality gates (workspace-wide, enforced by CI):

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Smoke test locally:

```sh
curl -i http://127.0.0.1:8080/health/ready
curl -i -H "Authorization: Bearer <supabase-access-token>" \
  http://127.0.0.1:8080/v1/me
curl -i -H "Authorization: Bearer <supabase-access-token>" \
  http://127.0.0.1:8080/v1/memories
```

## Supabase setup: Google sign-in (exact steps)

1. Create a Supabase project. Note the project URL
   (`https://<project-ref>.supabase.co`) and the project's **anon** key
   (Dashboard → Project Settings → API keys).
2. In the Google Cloud Console create an **OAuth 2.0 Client ID** of type
   **Web application** and capture its Client ID and Client secret. Use type
   *Web application*, not *Desktop*: Supabase exchanges the code with Google
   server-side using the client secret.
3. In the Google client's **Authorized redirect URIs** add exactly:
   `https://<project-ref>.supabase.co/auth/v1/callback`
   (You do **not** need to register loopback redirects with Google — see
   PKCE notes below.)
4. In the Supabase dashboard open **Authentication → Providers → Google**:
   enable the provider and paste the Client ID and Client secret. Save.
5. The scope request is minimal: `openid email profile`.
6. In **Authentication → URL Configuration → Redirect URLs**, allow the
   desktop loopback callback. Because the port is selected at runtime, use a
   narrowly scoped wildcard such as `http://127.0.0.1:*/oauth2/callback**` and
   verify it with Supabase's redirect glob tester before production rollout.

### Asymmetric signing key / JWKS requirement

The backend verifies JWTs **asymmetrically** (ES256 or RS256) using the
project's public keys. It must **never** hold the shared HS256 JWT secret.

- Dashboard → **Authentication → Signing Keys** → configure an asymmetric
  signing key with a stable `kid`.
- Supabase then serves the public keys at:
  `https://<project-ref>.supabase.co/auth/v1/.well-known/jwks.json`
- The backend caches this JWKS, validates the token `exp`/`iss`/`aud`/`sub`,
  and returns only the verified `sub`, `role`, `aal`, and `session_id` claims.
- If the project still issues HS256 tokens, the JWKS endpoint yields nothing
  usable and `GET /v1/me` will reject them — this is the fail-closed behavior
  to expect until the asymmetric key is configured.

### Redirect URL notes for desktop PKCE

The desktop app performs native **PKCE** sign-in against Supabase:

- It builds
  `{SUPABASE_URL}/auth/v1/authorize?provider=google&redirect_to=...&code_challenge=...&code_challenge_method=s256&scopes=openid email profile`.
- The redirect is a **loopback** URL to a port chosen at runtime:
  `http://127.0.0.1:<port>/oauth2/callback?state=...`
- That URL must match the Supabase redirect allow list described above. Google
  itself receives the fixed Supabase callback URL, not the loopback URL.
- Supabase allows `http` only for loopback/localhost hosts; the app binds the
  listener to `127.0.0.1` only and rejects connections from non-loopback peers.
- The callback is **not** a browser-based deep link: no custom URL scheme is
  registered, and no web server is involved beyond the one-shot loopback
  listener. The code is exchanged at `{SUPABASE_URL}/auth/v1/token?grant_type=pkce`
  with the **anon** key as `apikey`.
- The resulting `access_token` (the JWT) and `refresh_token` are stored in the
  macOS **Keychain**, not in the backend and not in the Backend env.
- If you change the callback path, the desktop listener and Supabase
  `redirect_to` must match exactly, and `state` must be preserved end to end.

## Database migration

`migrations/0001_profiles.sql` creates `public.profiles` keyed one-to-one to
`auth.users(id)` with an `updated_at` trigger, enables **and** forces RLS,
adds least-privilege `select`/`insert`/`update` policies scoped to
`auth.uid()`, and revokes the broad `anon`/`public` grants. There is no
`delete` policy, so a user can never remove their own row.

`migrations/0002_application_state.sql` adds tenant-scoped memories, todos,
mini-app records, agent runs/events, idempotency keys, devices, and upload
metadata with ownership policies and indexes. `0003_agent_leases.sql` adds
atomic worker leases, fencing tokens, retry scheduling, and encrypted
credential ciphertext. Raw provider keys are never stored; the Rust run store
strips `api_key` before durable persistence and clears ciphertext on terminal
settlement.

Apply all three from the Supabase dashboard (**SQL Editor**) or `supabase db push`
from `migrations/`. `GET /v1/profile` lazily creates the caller's profile row;
`PATCH /v1/profile` updates its bounded display/avatar fields.

## Production security boundary

- `STALKY_SUPABASE_PUBLISHABLE_KEY` is public by design and is used only by the
  desktop app for Supabase Auth. The backend needs only `SUPABASE_URL` because
  the project's JWKS endpoint is public.
- The `service_role` key **must never** be placed in the desktop app or the
  backend. It bypasses RLS, which would turn any compromise of those binaries
  into full read/write over every profile. If a future feature genuinely needs
  server-side admin access, it must be added through a separately justified,
  reviewed, least-privilege mechanism — never by shipping `service_role`.
- The backend stores no Supabase credentials at rest; configuration comes from
  the environment. Agent request API keys are encrypted with the server-only
  vault key for queued execution, never included in request payloads/events,
  and cleared after terminal settlement.
  Real `.env*` files are git-ignored; only `.env.example` templates are allowed
  into source control.

## Deployment and health checks

### Docker

```sh
# from the repository root
docker build -f backend/Dockerfile -t stalky-backend:latest .
docker run --rm -p 8080:8080 \
  -e SUPABASE_URL="https://<project-ref>.supabase.co" \
  -e DATABASE_URL="postgresql://<user>:<password>@<host>:5432/postgres" \
  -e RUST_LOG="info" \
  stalky-backend:latest
```

The image is multi-stage: it builds the backend on a pinned Rust image and
copies only the release binary into a non-root, minimal distroless image (not
`cargo-chef` based, which is optional). The container runs as UID 65532
(`nonroot`) and binds to `0.0.0.0:8080`.

### Orchestrator probes

Configure **HTTP** probes against the published port (the image has no shell
or curl, so use orchestrator-level probes):

- Liveness: `GET /health/live`, expect `200`.
- Readiness: `GET /health/ready`, expect `200`.
- Startup (optional): `GET /health/live`, expect `200`.

Set graceful shutdown via SIGTERM (handled by the binary). Example Kubernetes
probe:

```yaml
readinessProbe:
  httpGet: { path: /health/ready, port: 8080 }
```

## Privacy

Raw screen and audio data are **not persisted or transmitted** by this backend
slice. Screen frames and microphone PCM stay in the desktop process. The
backend does persist the explicit tenant resources listed above; raw provider
keys are not persisted. Remote transcription remains a future provider-adapter
milestone.

## Pricing

The backend adds no media-processing cost because it never receives screen or
audio content. As of August 2026, Supabase Free includes 50,000 MAU, 500 MB of
database, and 5 GB egress. Pro starts at $25/month with 100,000 MAU, then
$0.00325 per additional MAU; verify current figures at
<https://supabase.com/pricing> before launch.
