# A self-hostable receiver: "and your agent gets the bundle", without the Mac app

Status: plan, written 2026-10-22. Nothing here is built yet. It follows phase 8 (`scrawl-ffi`) in
`docs/library.md`.

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
   - ScrawlKit: with `compiles = .onDevice`, upload the finished bundle to `ScrawlConfig`'s
     receiver URL instead of the movie and `timeline.json`; the Mac app learns to accept a built
     bundle too, so both receivers take both.
2. **`scrawl-server`** (the receiver; a new binary crate, one static binary plus a Docker image).
   - `POST /bundles`: a zip of one `<id>.scrawl/`. Checks the token, the size (a limit, 200 MB by
     default), that every path stays inside the bundle and that `scrawl.json` parses as `version: 1`;
     stores it under `bundles/<id>.scrawl/`; replies as the ingest does.
   - `GET /bundles` and `GET /bundles/<id>/<file>`: the list and the files, same token, for the
     hand-offs below and for a person to look at.
   - Hands each new bundle to the configured hand-offs, through `scrawl-route`'s delivery queue so a
     failed send retries.
3. **Hand-offs** (pick any; each is optional).
   - **OpenCode**, on the agent host: the Mac app's prompt (summary text, contact sheet attached),
     to the session the upload names, else `smart::decide` on what the bundle records (URL,
     directory), else the newest root session. Same rules as `handoff::recorded_destination`: an
     explicit destination is never rerouted.
   - **MCP**, for any agent (Claude Code, OpenCode, Cursor): `scrawl mcp` (stdio) lists recent
     bundles and hands one over (summary text, contact sheet and frames as images, file paths). The
     agent pulls when the person says "look at my last scrawl", so nothing needs to know which
     session is open.
   - **Webhook**: a POST of the bundle's id, URL and summary to a configured address, for anything
     else (a chat bot, a ticket).

## Security and privacy

Bundles hold screenshots and voice. The receiver listens on loopback unless configured otherwise,
needs a token for every request, never logs bundle contents, and keeps bundles under the same
retention rules as the Mac app (`docs/export-and-retention.md`). A token in a public web page isn't
secret, so a deployed site should upload through its own backend, which holds the token.

## Order and size

| Step | What | Rough size |
|---|---|---|
| R1 | `scrawl-server`: `POST /bundles`, storage, list, ingest-shaped reply, tests with `IngestHooks`-style fakes | 2 days |
| R2 | `scrawl-web` `bundle.send`, against R1 in the browser test | 0.5 day |
| R3 | MCP hand-off (`scrawl mcp`) | 1 day |
| R4 | OpenCode hand-off through `scrawl-route`, webhook | 1 day |
| R5 | ScrawlKit upload of on-device bundles (after phase 8), Mac app accepts built bundles | 1 day |

## Decisions needed

1. **Which hand-off first:** MCP (works with any agent, the agent pulls) or OpenCode (pushes into a
   session, needs the agent host)?
2. **Where it runs by default:** the agent's own machine (simplest, OpenCode works) or a shared
   server (teams, needs forwarding for OpenCode)?
3. **Name:** `scrawl-server`, or folded into the `scrawl` CLI as `scrawl serve`?
