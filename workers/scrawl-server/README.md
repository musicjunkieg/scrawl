# scrawl-server on Cloudflare Workers

The receiver from `docs/receiver.md`: it takes `.scrawl` bundles uploaded with a token, keeps
them in an R2 bucket, and serves them back. All of its logic is in `crates/scrawl-server`; this
folder runs it as a Worker (Rust compiled to wasm with workers-rs).

## Deploy it (about 15 minutes the first time)

You need a Cloudflare account, Node, and Rust with the wasm target
(`rustup target add wasm32-unknown-unknown`). In this folder:

1. Sign in: `npx wrangler login`
2. Make the bucket: `npx wrangler r2 bucket create scrawl-bundles`
3. Set the token (any long random string; `openssl rand -hex 32` makes one) and keep a copy:
   `npx wrangler secret put SCRAWL_TOKEN`
4. Deploy: `npx wrangler deploy`. The first build installs `worker-build` (a few minutes).
5. Check it: opening `https://scrawl-server.<your-subdomain>.workers.dev/` says `scrawl-server`.

## Use it

Every request but `GET /` needs `Authorization: Bearer <token>`.

```sh
curl -H "Authorization: Bearer $TOKEN" --data-binary @web-1.scrawl.zip https://…/bundles
# {"ok":true,"delivered":false,"state":"saved","id":"web-1"}
curl -H "Authorization: Bearer $TOKEN" https://…/bundles               # newest first
curl -H "Authorization: Bearer $TOKEN" https://…/bundles/web-1/summary.md
```

The zip holds one `<id>.scrawl/` folder: what `scrawl-web`'s `bundle.zip()` makes, or a bundle
folder zipped in Finder. Uploads up to 40 MB are taken (`MAX_UPLOAD_BYTES` in `wrangler.toml`): a
Worker has 128 MB of memory and holds an upload about twice while unpacking it. A wrong token,
or a `Content-Length` over the limit, is turned away before the upload is read. A token in a public web page isn't
secret, so a deployed site should upload through its own backend.

## Run it on your own machine

`npx wrangler dev` runs it on `workerd`, Cloudflare's open-source runtime, with a local bucket.
Put `SCRAWL_TOKEN=…` in `.dev.vars` here first (git ignores it).
