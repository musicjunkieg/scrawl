# Scrawl web send test

A page that records itself with `scrawl-web` and sends the bundle to a `scrawl-server` through
its own backend (`server.mjs`, which uses `relayBundle`), the way a site would. It needs Node 22
or newer and Chrome (the page records its own tab with `preferCurrentTab`).

1. Build the compiler for the browser once (a few minutes):
   `rustup target add wasm32-unknown-unknown && cargo install --locked wasm-bindgen-cli --version 0.2.129`,
   then `packages/scrawl-web/scripts/build-wasm.sh` from the repo's root.
2. Start the server with your receiver's address and upload token, here from the receiver's
   secrets file:
   `set -a && source workers/scrawl-server/.secrets.env && set +a && SCRAWL_URL=https://scrawl-server.you.workers.dev node Examples/ScrawlWebSend/server.mjs`
   (`set -a` passes the file's values on to the server; a plain `source` keeps them in the shell).
3. Open http://127.0.0.1:8788 in Chrome, press Record and share the tab, click the counter, then
   press Stop and Send. The page says "Sent to your Scrawl server" with the bundle's id.

The token stays in the server's environment; the page never sees it. The server listens on this
Mac only.
