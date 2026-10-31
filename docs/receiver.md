# A self-hostable receiver: "and your agent gets the bundle", without the Mac app

Status: R1 and R3 built (2026-10-29): `crates/scrawl-server` and `workers/scrawl-server`, with
MCP at `/mcp`. R5's upload from ScrawlKit built for the iPhone (2026-10-30) and the Mac
(2026-10-31); the rest is planned. It follows phase 8 (`scrawl-ffi`) in `docs/library.md`.

## Why

The recorders no longer need the Mac app to make a bundle: `scrawl-web` builds one in the browser,
and ScrawlKit will on the phone once phase 8 lands. Getting that bundle to an agent still does.
Today the only receivers are the Mac app's ingest port (ScrawlKit uploads, `sdk.js` pages) and the
user carrying a downloaded zip. A web app's users, or a team without Macs, have no path at all.

```
 today                                         this plan
 ─────                                         ─────────
 scrawl-web ──zip in memory──> (you)           scrawl-web ─┐
 ScrawlKit  ──movie+timeline──> Mac app ─┐     ScrawlKit  ─┼─ POST bundle ─> scrawl-server ─┬─> OpenCode
 Mac capture ────────────────> Mac app ─┼─>                 │                               ├─> MCP (any agent)
                                         └─ opencode2 api   Mac app (unchanged) ────────────┘   └─> webhook
```

Think of the receiver as a mailroom. The recorders already write the letter (the bundle); the
mailroom signs for it (token check), files it (a bundles folder), and either walks it to the right
desk (OpenCode) or holds it at the front desk for whoever asks (MCP, a webhook ping).

## What exists to build on

- **The bundle is self-contained.** `scrawl-web`'s `zip()` and ScrawlKit's on-device compile
  produce the same `<id>.scrawl/` folder the Mac app writes, so a receiver only stores files.
- **`scrawl-route` is OS-free.** The delivery queue (`.deliver.json`, retries 15 s doubling to
  30 min for a day, `.deliver-failed.json`), the OpenCode session lookups and the routing decision
  (`smart::decide`, `ladder::route` with its `Hooks`) already compile on Linux and wasm.
- **The ingest protocol.** The Mac app's ScrawlKit ingest (`docs/iphone-and-routing.md`) already
  answers an upload with `{ok, delivered, state: "sent"|"queued"|"saved"|"processing",
  sessionTitle, error?}`, keyed by `recordedAt` so duplicate copies get the first's outcome. The
  receiver should speak the same reply so ScrawlKit's `IngestReply` works unchanged.

## The catch with OpenCode

The Mac app delivers by running `opencode2 api post /api/session/{id}/prompt` with the summary as
text and the contact sheet as a `file://` attachment. OpenCode reads that path from its own disk,
which is why the app copies bundles over SSH when the agents run on another Mac
(`handoff::send_bundle_to_session`). So OpenCode delivery needs the bundle on the agent host: the
receiver either runs there, or forwards to a small receiver that does. A receiver in the cloud can
still hold bundles for the other hand-offs.

## The pieces

1. **Upload from the recorders** (small).
   - `scrawl-web`: `bundle.send({ url, token, session? })` posts the zip (`application/zip`,
     `Authorization: Bearer <token>`) and resolves with the receiver's reply. The page decides
     whether to call it; nothing is sent otherwise, as today.
   - ScrawlKit (built for the iPhone): with `compiles = .onDevice` and a setup link to an HTTPS
     receiver, the finished bundle is zipped (`BundleZip`, stored, one `<id>.scrawl/` folder) into
     the outbox and sent to `POST /bundles` instead of the movie and `timeline.json` to
     `/ingest`, retried in the background like a hosted recording. The receiver's
     `{state: "saved"}` reads as "Sent to your Scrawl server". The token also goes in
     `X-Scrawl-Token`, since iOS may drop `Authorization` from a background upload, and
     `GET /health` answers ScrawlKit's token check. ScrawlKit for Mac does the same with
     `Scrawl.shared.receiver` (Help › Send Recordings to a Scrawl Server takes the setup link from
     the clipboard and keeps it in the Keychain), through an outbox on a regular session. Still to
     do: the Mac app accepting a built bundle, so both receivers take both.
2. **`scrawl-server`** (the receiver). `crates/scrawl-server` holds all of it with no OS bindings:
   `handle(config, request, store)` reads and writes through a `Store` trait. It runs as a
   Cloudflare Worker (`workers/scrawl-server`, workers-rs compiled to wasm, bundles in R2), and
   self-hosted on `workerd`, Cloudflare's open-source runtime, which runs in Docker. The Worker is
   its own Cargo workspace, so the app's offline builds never need workers-rs.
   - `POST /bundles`: a zip of one `<id>.scrawl/`, stored or deflated. Checks the token, the size
     (40 MB by default, zipped or unpacked: a Worker has 128 MB and holds an upload twice while
     unpacking it; a wrong token, an oversized `Content-Length` or none at all is refused before the
     body is read), that every path stays inside the bundle and that `scrawl.json` parses as `version: 1`;
     leaves out `__MACOSX/` and dot files; stores it under `bundles/<id>.scrawl/` (`scrawl.json` last, then
     `index/<newest first>-<id>` and `bundles/<id>.scrawl.json`); replies as the ingest does
     (`{ok: true, delivered: false, state: "saved", id}`). A bundle already there gets the same
     reply and isn't written again.
   - `GET /bundles` (newest first, `?limit=`), `GET /bundles/<id>` and `GET /bundles/<id>/<file>`:
     the list, one entry and the files, same token, for the hand-offs below and for a person to
     look at. `GET /` answers without a token, to check a deploy.
   - Hands each new bundle to the configured hand-offs, through `scrawl-route`'s delivery queue so a
     failed send retries.
3. **Hand-offs** (pick any; each is optional).
   - **OpenCode**, on the agent host: the Mac app's prompt (summary text, contact sheet attached),
     to the session the upload names, else `smart::decide` on what the bundle records (URL,
     directory), else the newest root session. Same rules as `handoff::recorded_destination`: an
     explicit destination is never rerouted.
   - **MCP**, for any agent (Claude Code, OpenCode, Cursor): the receiver's `/mcp`
     (`crates/scrawl-server/src/mcp.rs`; Streamable HTTP answered with plain JSON, no event
     stream or sessions; the same token; messages up to 1 MB) has three read-only tools:
     `list_scrawls` (the newest, as text and `structuredContent`), `get_scrawl` (the newest unless
     given an id: `summary.md` as text, the contact sheet as an image, and how to open a frame)
     and `get_scrawl_file` (a frame as an image, a text file as text; images over 5 MB aren't sent
     inline). Its `instructions` tell the agent to read the summary with the contact sheet. The
     agent pulls when the person says "look at my last scrawl", so nothing needs to know which
     session is open.
   - **Webhook**: a POST of the bundle's id, URL and summary to a configured address, for anything
     else (a chat bot, a ticket).

## Security and privacy

Bundles hold screenshots and voice. The receiver listens on loopback unless configured otherwise,
needs a token for every request, never logs bundle contents, and keeps bundles under the same
retention rules as the Mac app (`docs/export-and-retention.md`). A web page uploads through its own
backend (the receiver answers no CORS preflight). A token in a shipped app isn't secret, so the
receiver has two: `SCRAWL_TOKEN` reads and sends everything (the
owner and their agent), and `SCRAWL_UPLOAD_TOKEN` only sends bundles (`POST /bundles`,
`GET /health`; anything else is 403), so the one an app carries can add bundles but not read
anyone's. On Workers its uploads are limited to 20 a minute from one address (a rate-limit binding;
429 past it), so a token pulled out of an app can't fill the bucket quickly.

## Order and size

| Step | What | Rough size |
|---|---|---|
| R1 | `scrawl-server`: `POST /bundles`, storage, list, ingest-shaped reply, tests with a memory store; the Worker on R2 (done) | 2 days |
| R2 | `scrawl-web` `bundle.send`, against R1 in the browser test | 0.5 day |
| R3 | MCP hand-off at `/mcp` on the receiver (done) | 1 day |
| R4 | OpenCode hand-off through `scrawl-route`, webhook | 1 day |
| R5 | ScrawlKit upload of on-device bundles (iPhone and Mac done), Mac app accepts built bundles | 1 day |

## Decisions (2026-10-29)

1. **MCP first:** any agent pulls, so nothing needs to know which session is open. The receiver
   serves MCP itself (Streamable HTTP at `/mcp`, same token), so an agent adds it once by URL.
2. **A shared server by default,** not a process on the agent's machine: Cloudflare Workers first,
   self-hosted `workerd` (Docker) for anyone else. OpenCode delivery (R4) needs the bundle on the
   agent host, so it forwards there.
3. **`scrawl-server`,** its own crate: the `scrawl` CLI is the Mac app and only builds on macOS and
   Windows.
