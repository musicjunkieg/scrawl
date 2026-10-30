# scrawl-server on Cloudflare Workers

The receiver from `docs/receiver.md`: it takes `.scrawl` bundles uploaded with a token, keeps
them in an R2 bucket, and serves them back, to people over HTTP and to agents over MCP. All of its logic is in `crates/scrawl-server`; this
folder runs it as a Worker (Rust compiled to wasm with workers-rs).

## Deploy it (about 15 minutes the first time)

You need a Cloudflare account, Node 22.23+, 24.22+ or 26.12+ (what `cf` supports) with pnpm, and
Rust with the wasm target (`rustup target add wasm32-unknown-unknown`). It deploys with `cf`,
Cloudflare's CLI (in beta; it runs Wrangler's bundler for the Rust build). In this folder:

1. Install the tools: `pnpm install`
2. Sign in: `pnpm cf auth login`
3. Make the two tokens (long random strings) in `.secrets.env`, which git ignores:

   ```sh
   printf 'SCRAWL_TOKEN=%s\nSCRAWL_UPLOAD_TOKEN=%s\n' "$(openssl rand -hex 32)" "$(openssl rand -hex 32)" > .secrets.env
   ```

   `SCRAWL_TOKEN` reads and sends everything: keep it for yourself and your agent.
   `SCRAWL_UPLOAD_TOKEN` can only send bundles: it's the one to put in an app, since anyone can
   pull a token out of an app, and with it they can add bundles but not read any.
4. Deploy: `pnpm run deploy` (not `pnpm deploy`, which is pnpm's own command). It uploads both
   tokens with the Worker, and makes the `scrawl-bundles` bucket the first time. The first build
   installs `worker-build` (a few minutes).
5. Check it: opening `https://scrawl-server.<your-subdomain>.workers.dev/` says `scrawl-server`.

Deploy again with `pnpm run deploy` after pulling changes; it sends the token from `.secrets.env`
each time.

## Use it

Every request but `GET /` needs `Authorization: Bearer <token>` (or `X-Scrawl-Token: <token>`).
The upload token gets `POST /bundles` and `GET /health` only; anything else answers 403.

```sh
curl -H "Authorization: Bearer $TOKEN" --data-binary @web-1.scrawl.zip https://…/bundles
# {"ok":true,"delivered":false,"state":"saved","id":"web-1"}
curl -H "Authorization: Bearer $TOKEN" https://…/bundles               # newest first
curl -H "Authorization: Bearer $TOKEN" https://…/bundles/web-1/summary.md
```

The zip holds one `<id>.scrawl/` folder: what `scrawl-web`'s `bundle.zip()` makes, or a bundle
folder zipped in Finder. Uploads up to 40 MB are taken (`MAX_UPLOAD_BYTES` in `cloudflare.config.ts`): a
Worker has 128 MB of memory and holds an upload about twice while unpacking it. A wrong token,
a `Content-Length` over the limit, or no `Content-Length` at all, is turned away before the
upload is read. A token in a shipped app isn't secret: give an app the upload token, never
`SCRAWL_TOKEN`. A web page can't upload here directly (the receiver answers no CORS preflight), so
a site uploads through its own backend, which holds the token. Anyone who pulls the upload token
out of an app can add bundles, though not read them: if that happens, set a new
`SCRAWL_UPLOAD_TOKEN`, deploy, and ship the app with it.

## Give it to your agent (MCP)

The receiver is also an MCP server at `/mcp`, with the same token. An agent added once can list
the recordings, look at one (its `summary.md` and contact sheet) and open a frame at full size, so
"look at my last scrawl" works in any session.

- **Claude Code:**
  `claude mcp add --transport http scrawl https://scrawl-server.<your-subdomain>.workers.dev/mcp --header "Authorization: Bearer $TOKEN"`
- **Cursor** (`~/.cursor/mcp.json`) and others that take a URL and headers:
  `{"mcpServers": {"scrawl": {"url": "https://…/mcp", "headers": {"Authorization": "Bearer …"}}}}`
- **OpenCode** (`opencode.json`):
  `{"mcp": {"scrawl": {"type": "remote", "url": "https://…/mcp", "headers": {"Authorization": "Bearer …"}}}}`

Its tools are `list_scrawls`, `get_scrawl` (the newest unless given an id) and `get_scrawl_file`.

## Run it on your own machine

`pnpm run dev` runs it on `workerd`, Cloudflare's open-source runtime, with a local bucket.
Put `SCRAWL_TOKEN=…` (and `SCRAWL_UPLOAD_TOKEN=…` to try it) in `.dev.vars` here first (git
ignores it).
