// A test page for scrawl-web's bundle.send: records itself, posts the bundle to /api/scrawl, and
// this server passes it on to your scrawl-server with relayBundle, the way a site's backend would.
//   SCRAWL_URL=https://scrawl-server.you.workers.dev SCRAWL_UPLOAD_TOKEN=… node server.mjs
// Then open http://localhost:8788 in Chrome. Listens on this Mac only.
import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { relayBundle } from "../../packages/scrawl-web/src/relay.js";

const url = process.env.SCRAWL_URL;
const token = process.env.SCRAWL_UPLOAD_TOKEN;
if (!url || !token) {
  console.error("Set SCRAWL_URL (your receiver) and SCRAWL_UPLOAD_TOKEN (its upload token).");
  process.exit(1);
}

const here = dirname(fileURLToPath(import.meta.url));
const web = resolve(here, "../../packages/scrawl-web");
const types = { ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm" };

async function relay(req, res) {
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  const request = new Request(`http://localhost${req.url}`, {
    method: req.method,
    headers: req.headers,
    body: Buffer.concat(chunks),
  });
  // A real site checks here that this person may send recordings.
  const reply = await relayBundle(request, { url, token });
  console.log(`relayed ${chunks.reduce((n, c) => n + c.length, 0)} bytes → ${reply.status}`);
  res.writeHead(reply.status, { "Content-Type": reply.headers.get("content-type") });
  res.end(Buffer.from(await reply.arrayBuffer()));
}

createServer((req, res) => {
  const path = new URL(req.url, "http://x").pathname;
  if (req.method === "POST" && path === "/api/scrawl") {
    return void relay(req, res).catch((error) => {
      console.error(error);
      res.writeHead(502, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ ok: false, error: `The receiver couldn't be reached: ${error.message}` }));
    });
  }
  // The page, and scrawl-web's own files (src/, wasm/).
  const file = path === "/" ? join(here, "index.html") : join(web, decodeURIComponent(path));
  if (!file.startsWith(here) && !file.startsWith(web)) return void res.writeHead(404).end();
  let body;
  try {
    body = readFileSync(file);
  } catch {
    return void res.writeHead(404).end();
  }
  res.writeHead(200, { "Content-Type": types[extname(file)] || "application/octet-stream" });
  res.end(body);
}).listen(8788, "127.0.0.1", () => console.log(`Open http://localhost:8788 in Chrome (sending to ${url})`));
