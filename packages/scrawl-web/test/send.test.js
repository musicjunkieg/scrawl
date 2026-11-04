// bundle.send and relayBundle against a stand-in for scrawl-server's POST /bundles.
import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

import { relayBundle } from "../src/relay.js";
import { sendZip } from "../src/send.js";
import { zip } from "../src/zip.js";

const realFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = realFetch;
});

const saved = { ok: true, delivered: false, state: "saved", id: "web-1" };
const bundleZip = () =>
  new Blob([zip([["summary.md", new TextEncoder().encode("hi\n")]], "web-1.scrawl")], { type: "application/zip" });

/** The request as the page's backend gets it: the network adds the body's Content-Length. */
async function arriving(url, init) {
  const body = new Uint8Array(await new Response(init.body).arrayBuffer());
  const headers = new Headers(init.headers);
  headers.set("content-length", String(body.length));
  return new Request(new URL(url, "https://app.example"), { ...init, headers, body });
}

/** A receiver that wants `token` and keeps what it was sent. */
function receiver(token) {
  const got = [];
  const answer = async (url, init) => {
    const request = new Request(url, init);
    got.push({ url: request.url, auth: request.headers.get("authorization"), body: new Uint8Array(await request.arrayBuffer()) });
    if (request.headers.get("authorization") !== `Bearer ${token}`) {
      return Response.json({ ok: false, error: "That token isn't right" }, { status: 401 });
    }
    return Response.json(saved);
  };
  return { got, fetch: answer };
}

test("bundle.send posts the zip and relayBundle passes it on with the upload token", async () => {
  const server = receiver("up-123");
  // The page's backend: its own check would go here, then the relay.
  globalThis.fetch = async (url, init) =>
    relayBundle(await arriving(url, init), {
      url: "https://scrawl-server.example.workers.dev",
      token: "up-123",
      fetch: server.fetch,
    });
  const zipped = bundleZip();
  const reply = await sendZip(zipped, { url: "/api/scrawl" });
  assert.deepEqual(reply, saved);
  assert.equal(server.got.length, 1);
  assert.equal(server.got[0].url, "https://scrawl-server.example.workers.dev/bundles");
  assert.equal(server.got[0].auth, "Bearer up-123");
  assert.deepEqual(server.got[0].body, new Uint8Array(await zipped.arrayBuffer()));
});

test("a refusal comes back as an error with the status and reply", async () => {
  const server = receiver("right");
  globalThis.fetch = async (url, init) =>
    relayBundle(await arriving(url, init), {
      url: "https://r.example/base/",
      token: "wrong",
      fetch: server.fetch,
    });
  await assert.rejects(sendZip(bundleZip(), { url: "/api/scrawl" }), (error) => {
    assert.equal(error.status, 401);
    assert.equal(error.reply.error, "That token isn't right");
    assert.match(error.message, /401.*That token isn't right/);
    return true;
  });
  assert.equal(server.got[0].url, "https://r.example/base/bundles");
});

test("the relay refuses what isn't a bundle before reading it or calling the receiver", async () => {
  const server = receiver("t");
  const options = { url: "https://r.example", token: "t", maxBytes: 10, fetch: server.fetch };
  const post = async (headers, body = new Uint8Array(4)) =>
    relayBundle(await arriving("/api/scrawl", { method: "POST", headers, body }), options);
  assert.equal((await relayBundle(new Request("https://app.example/api/scrawl"), options)).status, 405);
  assert.equal((await post({ "Content-Type": "text/plain" })).status, 415);
  const big = await post({ "Content-Type": "application/zip" }, new Uint8Array(11));
  assert.equal(big.status, 413);
  assert.match((await big.json()).error, /over/);
  const unsized = new Request("https://app.example/api/scrawl", {
    method: "POST",
    headers: { "Content-Type": "application/zip" },
    body: new Uint8Array(4),
  });
  assert.equal((await relayBundle(unsized, options)).status, 411);
  assert.equal(server.got.length, 0);
  assert.equal((await post({ "Content-Type": "application/zip; charset=binary" })).status, 200);
  assert.equal(server.got.length, 1);
});

test("send needs a url", async () => {
  await assert.rejects(sendZip(bundleZip(), {}), /needs a url/);
  await assert.rejects(relayBundle(new Request("https://a.example"), { url: "https://r.example" }), /upload token/);
});
