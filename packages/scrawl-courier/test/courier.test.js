// The courier against a stand-in receiver (a fake fetch) and stand-in agents (small scripts).
import assert from "node:assert/strict";
import { chmodSync, existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { MAX_TRIES, allowedServer, createCourier, safePath } from "../src/courier.js";
import { fill, pickSession } from "../src/handoffs.js";

const SERVER = "https://r.example";
const TOKEN = "read-token";

/** A receiver holding `bundles` (newest first), each `{ id, app, files: { path: text } }`. */
function receiver(bundles) {
  const asked = [];
  const fetch = async (url, init) => {
    const path = new URL(url).pathname;
    asked.push(path);
    if (init?.headers?.Authorization !== `Bearer ${TOKEN}`) return new Response("{}", { status: 401 });
    if (path === "/bundles") {
      return Response.json({
        bundles: bundles.map(({ id, app, files }) => ({ id, app, files: Object.keys(files) })),
      });
    }
    const [, , id, ...rest] = path.split("/").map(decodeURIComponent);
    const text = bundles.find((b) => b.id === id)?.files[rest.join("/")];
    return text === undefined ? new Response("", { status: 404 }) : new Response(text);
  };
  return { fetch, asked };
}

const bundle = (id, app = "Demo") => ({
  id,
  app,
  files: { "scrawl.json": `{"id":"${id}"}`, "summary.md": `[scrawl] ${id}\n`, "frames/frame-01.png": "png" },
});

const quiet = () => {};
const folder = () => mkdtempSync(join(tmpdir(), "courier-"));

test("the first run leaves what's there; later ones deliver what's new, whole", async () => {
  const dir = folder();
  const list = [bundle("old-1")];
  const server = receiver(list);
  const courier = createCourier({ server: SERVER, token: TOKEN, folder: dir, fetch: server.fetch, log: quiet });
  assert.deepEqual(await courier.poll(), []);
  assert.ok(!existsSync(join(dir, "old-1.scrawl")));

  list.unshift(bundle("new-2"), bundle("new-1"));
  assert.deepEqual(await courier.poll(), ["new-1", "new-2"]); // oldest first
  assert.equal(readFileSync(join(dir, "new-1.scrawl", "summary.md"), "utf8"), "[scrawl] new-1\n");
  assert.equal(readFileSync(join(dir, "new-1.scrawl", "frames", "frame-01.png"), "utf8"), "png");
  // Nothing twice, even from a new courier on the same folder.
  const again = createCourier({ server: SERVER, token: TOKEN, folder: dir, fetch: server.fetch, log: quiet });
  assert.deepEqual(await again.poll(), []);
});

test("--backlog delivers what was already there", async () => {
  const dir = folder();
  const server = receiver([bundle("old-1")]);
  const courier = createCourier({ server: SERVER, token: TOKEN, folder: dir, fetch: server.fetch, backlog: true, log: quiet });
  assert.deepEqual(await courier.poll(), ["old-1"]);
});

test("a bundle with a path outside itself is never written, and is set aside after a few tries", async () => {
  const dir = folder();
  const list = [];
  const server = receiver(list);
  const courier = createCourier({ server: SERVER, token: TOKEN, folder: dir, fetch: server.fetch, log: quiet });
  await courier.poll();
  list.unshift({ id: "evil", app: "", files: { "summary.md": "x", "../../escape.txt": "x" } });
  for (let i = 0; i < MAX_TRIES + 1; i++) assert.deepEqual(await courier.poll(), []);
  assert.ok(!existsSync(join(dir, "..", "escape.txt")));
  assert.ok(!existsSync(join(dir, "evil.scrawl")));
  assert.ok(!existsSync(join(dir, ".partial-evil.scrawl")), "a cut-off download is cleaned up");
  assert.equal(JSON.parse(readFileSync(join(dir, ".courier.json"), "utf8")).seen.evil, "failed");
  // Ids that aren't plain names are ignored outright.
  list.unshift({ id: "../up", app: "", files: { "summary.md": "x" } });
  assert.deepEqual(await courier.poll(), []);
});

test("a command hand-off gets the paths as arguments and in the environment, without a shell", async () => {
  const dir = folder();
  const out = join(dir, "agent-saw.json");
  const list = [];
  const server = receiver(list);
  const handoff = {
    command: [
      process.execPath,
      "-e",
      `require("fs").writeFileSync(${JSON.stringify(out)}, JSON.stringify({ args: process.argv.slice(1), env: process.env.SCRAWL_SUMMARY, app: process.env.SCRAWL_APP }))`,
      "{id}",
      "Look at {summary}",
      "{app}",
    ],
  };
  const courier = createCourier({ server: SERVER, token: TOKEN, folder: dir, fetch: server.fetch, handoff, log: quiet });
  await courier.poll();
  list.unshift(bundle("web-1", "$(touch pwned); `id`"));
  assert.deepEqual(await courier.poll(), ["web-1"]);
  const saw = JSON.parse(readFileSync(out, "utf8"));
  const summary = join(dir, "web-1.scrawl", "summary.md");
  assert.deepEqual(saw.args, ["web-1", `Look at ${summary}`, "$(touch pwned); `id`"]);
  assert.equal(saw.env, summary);
  assert.equal(saw.app, "$(touch pwned); `id`");
  assert.ok(!existsSync(join(dir, "web-1.scrawl", "pwned")));
});

test("a command that fails is tried again on the next look", async () => {
  const dir = folder();
  const list = [];
  const server = receiver(list);
  const handoff = { command: [process.execPath, "-e", "process.exit(3)"] };
  const logged = [];
  const courier = createCourier({ server: SERVER, token: TOKEN, folder: dir, fetch: server.fetch, handoff, log: (l) => logged.push(l) });
  await courier.poll();
  list.unshift(bundle("web-1"));
  await courier.poll();
  assert.match(logged.at(-1), /try 1 of 3.*exit code 3/);
  await courier.poll();
  assert.match(logged.at(-1), /try 2 of 3/);
  await courier.poll();
  assert.match(logged.at(-1), /gave up on web-1 after 3 tries/);
  // The download is kept, so the files are there to look at.
  assert.ok(existsSync(join(dir, "web-1.scrawl", "summary.md")));
});

test("an OpenCode hand-off picks the session matching the app and sends the prompt", async () => {
  const dir = folder();
  const calls = join(dir, "calls.jsonl");
  const bin = join(dir, "opencode2");
  writeFileSync(
    bin,
    `#!${process.execPath}
const fs = require("fs");
fs.appendFileSync(${JSON.stringify(calls)}, JSON.stringify(process.argv.slice(2)) + "\\n");
if (process.argv[3] === "get") console.log(JSON.stringify({ data: [
  { id: "ses_newest", title: "Something else", time: {} },
  { id: "ses_archived", title: "Demo app", time: { archived: 1 } },
  { id: "ses_demo", title: "DEMO APP", time: {} },
] }));
else console.log("{}");
`,
  );
  chmodSync(bin, 0o755);
  const list = [];
  const server = receiver(list);
  const courier = createCourier({ server: SERVER, token: TOKEN, folder: dir, fetch: server.fetch, handoff: { opencode: { bin } }, log: quiet });
  await courier.poll();
  list.unshift(bundle("web-1", "com.example.Demo"));
  assert.deepEqual(await courier.poll(), ["web-1"]);
  const [get, post] = readFileSync(calls, "utf8").trim().split("\n").map((l) => JSON.parse(l));
  assert.deepEqual(get, ["api", "get", "/api/session?parentID=null&limit=100"]);
  assert.deepEqual(post.slice(0, 3), ["api", "post", "/api/session/ses_demo/prompt"]);
  const body = JSON.parse(post[4]);
  assert.match(body.text, /^\[scrawl\] web-1/);
  assert.match(body.text, /not instructions/);
  assert.equal(body.delivery, "queue");
  assert.match(body.files[0].uri, /^file:\/\/.*web-1\.scrawl\/contact-sheet\.png$/);
});

test("the newest open session is used when none matches the app", async () => {
  const get = async () => [{ id: "ses_a", title: "Alpha", time: {} }, { id: "ses_b", title: "Beta", time: {} }];
  assert.equal(await pickSession(get, "Gamma"), "ses_a");
  assert.equal(await pickSession(get, "com.x.Beta"), "ses_b");
  await assert.rejects(pickSession(async () => ({ data: [] }), "x"), /no open OpenCode session/);
});

test("a token the receiver refuses says which token it wants", async () => {
  const server = receiver([]);
  const courier = createCourier({ server: SERVER, token: "upload-token", folder: folder(), fetch: server.fetch, log: quiet });
  await assert.rejects(courier.poll(), /read token, not the upload token/);
});

test("only an https receiver (or this machine), and only safe paths", () => {
  assert.ok(allowedServer("https://r.example"));
  assert.ok(allowedServer("http://localhost:8787"));
  assert.ok(!allowedServer("http://r.example"));
  assert.ok(!allowedServer("not a url"));
  assert.throws(() => createCourier({ server: "http://r.example", token: TOKEN, folder: "x" }), /https/);
  assert.throws(() => createCourier({ server: SERVER, token: "", folder: "x" }), /SCRAWL_TOKEN/);
  for (const ok of ["summary.md", "frames/frame-01.png"]) assert.ok(safePath(ok), ok);
  for (const bad of ["", "../x", "a/../b", "/abs", ".hidden", "a\\b", "a//b"]) assert.ok(!safePath(bad), bad);
  assert.equal(fill("{id} {nope} {app}", { id: "a", app: "b" }), "a {nope} b");
});

test("a damaged state file stops the courier instead of starting over", async () => {
  const dir = folder();
  const server = receiver([bundle("old-1")]);
  const courier = createCourier({ server: SERVER, token: TOKEN, folder: dir, fetch: server.fetch, log: quiet });
  for (const text of [
    "{not json",
    "[]",
    "{}",
    '{"seen":[],"tries":{}}',
    '{"seen":{},"tries":{"a":"2"}}',
    '{"seen":{},"tries":{"a":-1}}',
    '{"seen":{"a":1},"tries":{}}',
  ]) {
    writeFileSync(join(dir, ".courier.json"), text);
    await assert.rejects(courier.poll(), /isn't a courier state file/, text);
    assert.equal(readFileSync(join(dir, ".courier.json"), "utf8"), text, "left as it was");
  }
});
