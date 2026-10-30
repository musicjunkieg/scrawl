# scrawl-server on Cloudflare Workers

The receiver from `docs/receiver.md`: it takes `.scrawl` bundles uploaded with a token, keeps
them in an R2 bucket, and serves them back, to people over HTTP and to agents over MCP. All of its logic is in `crates/scrawl-server`; this
folder runs it as a Worker (Rust compiled to wasm with workers-rs).

## Deploy it (about 15 minutes the first time)

You need a Cloudflare account, Node, and Rust with the wasm target
(`rustup target add wasm32-unknown-unknown`). It deploys with `cf`, Cloudflare's CLI (in beta; it
runs Wrangler's bundler for the Rust build). In this folder:

1. Install the tools: `npm install`
2. Sign in: `npx cf auth login`
3. Make the token (any long random string) and keep a copy:
   `echo "SCRAWL_TOKEN=$(openssl rand -hex 32)" > .secrets.env`. Git ignores this file.
4. Deploy: `npm run deploy`. It uploads the token with the Worker, and makes the
   `scrawl-bundles` bucket the first time. The first build installs `worker-build` (a few minutes).
5. Check it: opening `https://scrawl-server.<your-subdomain>.workers.dev/` says `scrawl-server`.

Deploy again with `npm run deploy` after pulling changes; it sends the token from `.secrets.env`
each time.

## Use it

Every request but `GET /` needs `Authorization: Bearer <token>`.

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
upload is read. A token in a public web page isn't
secret, so a deployed site should upload through its own backend.

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

`npm run dev` runs it on `workerd`, Cloudflare's open-source runtime, with a local bucket.
Put `SCRAWL_TOKEN=…` in `.dev.vars` here first (git ignores it).
