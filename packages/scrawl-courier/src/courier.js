// The courier: asks a scrawl-server for recordings it hasn't delivered, downloads each into a
// folder, and hands it on (`handoffs.js`). It asks rather than being rung because a laptop can't
// be reached from the cloud. What it has delivered is kept in `<folder>/.courier.json`, so a
// restart picks up where it left off and nothing goes twice.

import { mkdir, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
import { join } from "node:path";

import { handoffFor } from "./handoffs.js";

/** Tries before a recording that keeps failing to hand off is set aside (and logged). */
export const MAX_TRIES = 3;

/** A bundle id as the receiver keeps them: no path separators, no leading dot. */
const SAFE_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,199}$/;

/** A file path inside a bundle: relative, `/`-separated, nothing hidden or climbing out. */
export function safePath(path) {
  return (
    typeof path === "string" &&
    path.length > 0 &&
    !path.includes("\\") &&
    path.split("/").every((part) => part && part !== "." && part !== ".." && !part.startsWith("."))
  );
}

/** The receiver's address over HTTPS, or plain HTTP only to this machine. */
export function allowedServer(server) {
  let url;
  try {
    url = new URL(server);
  } catch {
    return false;
  }
  if (url.protocol === "https:") return true;
  return url.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
}

/**
 * @param {object} options
 * @param {string} options.server The receiver, `https://scrawl-server.you.workers.dev`.
 * @param {string} options.token Its read token (`SCRAWL_TOKEN`): listing needs it.
 * @param {string} options.folder Where recordings are downloaded, one `<id>.scrawl/` each.
 * @param {string | object} [options.handoff] `"folder"`, `{ command: [...] }` or `{ opencode: {...} }`.
 * @param {boolean} [options.backlog] On the first run, deliver what's already there too
 *   (by default it's noted as seen, so an agent isn't handed the whole history).
 */
export function createCourier({ server, token, folder, handoff = "folder", backlog = false, fetch: get = fetch, log = console.log }) {
  if (!allowedServer(server)) throw new Error("server must be an https:// address (or http:// to this machine)");
  if (!token) throw new Error("set SCRAWL_TOKEN to the receiver's token");
  const base = server.endsWith("/") ? server : `${server}/`;
  const statePath = join(folder, ".courier.json");
  const deliver = handoffFor(handoff);

  async function request(path) {
    const response = await get(new URL(path, base), { headers: { Authorization: `Bearer ${token}` } });
    if (response.status === 401 || response.status === 403) {
      throw new Error("the receiver refused the token: SCRAWL_TOKEN must be its read token, not the upload token");
    }
    if (!response.ok) throw new Error(`the receiver answered ${response.status} for ${path}`);
    return response;
  }

  /** The saved state, or null on a first run (no file yet). A file that can't be read or isn't
   * the courier's stops it, rather than being taken for a first run and losing what was pending. */
  async function readState() {
    let text;
    try {
      text = await readFile(statePath, "utf8");
    } catch (error) {
      if (error.code === "ENOENT") return null;
      throw error;
    }
    const plain = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
    let state = null;
    try {
      state = JSON.parse(text);
    } catch {}
    const valid =
      plain(state) &&
      plain(state.seen) &&
      plain(state.tries) &&
      Object.values(state.seen).every((value) => typeof value === "string") &&
      Object.values(state.tries).every((value) => Number.isSafeInteger(value) && value >= 0);
    if (!valid) {
      throw new Error(`${statePath} isn't a courier state file; fix or delete it (deleting it starts over)`);
    }
    return state;
  }

  async function saveState(state) {
    await writeFile(`${statePath}.tmp`, JSON.stringify(state, null, 2));
    await rename(`${statePath}.tmp`, statePath);
  }

  /** Downloads one bundle into `<folder>/<id>.scrawl/` (whole, or not at all) and returns its folder. */
  async function download(entry) {
    const dir = join(folder, `${entry.id}.scrawl`);
    if (await stat(dir).catch(() => null)) return dir;
    const partial = join(folder, `.partial-${entry.id}.scrawl`);
    await rm(partial, { recursive: true, force: true });
    try {
      for (const file of entry.files) {
        if (!safePath(file)) throw new Error(`a file name isn't allowed: ${file}`);
        const path = `bundles/${encodeURIComponent(entry.id)}/${file.split("/").map(encodeURIComponent).join("/")}`;
        const bytes = new Uint8Array(await (await request(path)).arrayBuffer());
        const target = join(partial, ...file.split("/"));
        await mkdir(join(target, ".."), { recursive: true });
        await writeFile(target, bytes);
      }
      await rename(partial, dir);
    } catch (error) {
      await rm(partial, { recursive: true, force: true });
      throw error;
    }
    return dir;
  }

  /** One look at the receiver: delivers what's new, oldest first. Returns the ids delivered. */
  async function poll() {
    await mkdir(folder, { recursive: true });
    const { bundles = [] } = await (await request("bundles?limit=50")).json();
    const entries = bundles.filter((entry) => SAFE_ID.test(entry?.id ?? "") && Array.isArray(entry.files)).reverse();
    let state = await readState();
    if (!state) {
      state = { seen: {}, tries: {} };
      if (!backlog) {
        for (const entry of entries) state.seen[entry.id] = "before";
        await saveState(state);
        log(`courier: ${entries.length} recording(s) already there are left as they are; new ones will be delivered`);
        return [];
      }
    }
    const delivered = [];
    for (const entry of entries) {
      if (state.seen[entry.id]) continue;
      try {
        const dir = await download(entry);
        await deliver(bundleAt(dir, entry));
        state.seen[entry.id] = "delivered";
        delete state.tries[entry.id];
        delivered.push(entry.id);
        log(`courier: delivered ${entry.id}`);
      } catch (error) {
        const tries = (state.tries[entry.id] ?? 0) + 1;
        state.tries[entry.id] = tries;
        if (tries >= MAX_TRIES) {
          state.seen[entry.id] = "failed";
          delete state.tries[entry.id];
          log(`courier: gave up on ${entry.id} after ${tries} tries: ${error.message}`);
        } else {
          log(`courier: ${entry.id} not delivered (try ${tries} of ${MAX_TRIES}): ${error.message}`);
        }
      }
      await saveState(state);
    }
    return delivered;
  }

  return { poll };
}

/** What a hand-off is given: the bundle's folder and its main files. */
export function bundleAt(dir, entry) {
  return {
    id: entry.id,
    app: entry.app ?? "",
    dir,
    summary: join(dir, "summary.md"),
    contactSheet: join(dir, "contact-sheet.png"),
    scrawlJson: join(dir, "scrawl.json"),
    entry,
  };
}
