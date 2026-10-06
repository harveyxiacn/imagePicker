# Security Policy

imagePicker runs locally and binds to 127.0.0.1 by default. If you find a vulnerability (e.g. path traversal through the API, LAN-mode auth bypass), please report it privately via GitHub Security Advisories ("Report a vulnerability") rather than a public issue.

## Security model (summary)

Contract: [docs/api-contract-m6.md §B](docs/api-contract-m6.md). Implementation: `crates/ip-server/src/auth.rs`, `crates/ip-core/src/{auth,roots}.rs`.

- **Authentication.** Every `/api/*` route except `/api/health` and `/api/auth/*` needs a credential; WebSocket `/api/events` too.
  - Desktop: the shell generates a 256-bit per-launch token and opens `http://127.0.0.1:<port>/?token=...` once. The server answers `303` with an `HttpOnly; SameSite=Strict` session cookie and strips the token from the URL. The token never reaches page JavaScript. `Authorization: Bearer <token>` and `?token=` on API calls are also accepted.
  - LAN (`imagepicker serve --lan --password ...` or enabled in settings): `POST /api/auth/login` issues a session cookie. Passwords are stored as Argon2id hashes in `<data_dir>/security.json`. LAN mode refuses to start without an owner password. Optional read-only **guest** password.
  - Development (`imagepicker serve` without a token): requests from a genuine loopback client (no proxy headers, loopback `Host`) need no credentials; all other clients get 401. Binding a non-loopback host requires `--lan`.
- **Sessions.** 256-bit random ids (stored hashed), 7 days sliding expiry, invalidated on password change, constant-time secret comparison, login rate limit of 5 failures per minute per IP (HTTP 429).
- **Roles.** Guests are read-only (no non-GET requests except logout) and get 403 on people, faces, face crops, taste, settings, assistant, filesystem and system endpoints.
- **Browser safety.** No CORS headers (same-origin only; the Vite dev origins `localhost:5173`/`127.0.0.1:5173` are allowed only with `IMAGEPICKER_DEV_CORS=1`). State-changing requests and WebSocket upgrades carrying a foreign `Origin` are refused (CSRF / cross-site WebSocket hijacking). Unauthenticated loopback access additionally requires a loopback `Host` header (DNS-rebinding defence).
- **Filesystem whitelist.** `/api/fs/*`, `/api/import`, export `dest` and LUT import `path` must resolve (canonicalised; no `..`, no symlink/junction escapes, no UNC/device/verbatim/ADS/reserved names on Windows) inside: the user home, Pictures, removable drives, folders chosen natively in the desktop app, previously imported roots and `roots` from `security.json`. Otherwise `403 forbidden_path`.

Known limits: LAN traffic is plain HTTP (use a trusted network, or a TLS reverse proxy, which must send `X-Forwarded-For` so its loopback connections are not trusted as local); a guest sees photo metadata and originals but no people/face data; the whitelist covers paths supplied through the HTTP API, not the CLI.
