// For the page's backend: takes the zip `bundle.send` posts and passes it to a scrawl-server with
// the receiver's upload token, so the token stays on the server. A Fetch API handler (Workers,
// Next.js route handlers, Deno, Bun, Hono): a `Request` in, a `Response` out. Check that the
// person may send before calling it; this only forwards.

/** The receiver's default limit (`MAX_UPLOAD_BYTES`), zipped. */
export const MAX_UPLOAD_BYTES = 40 * 1024 * 1024;

/**
 * Forwards `request`'s zip to `<url>/bundles` and answers with the receiver's reply (its status,
 * so a 429 from the receiver's rate limit reaches the page). Refuses before reading the body
 * when it isn't a POST of `application/zip` with a `Content-Length` of at most `maxBytes`.
 */
export async function relayBundle(request, { url, token, maxBytes = MAX_UPLOAD_BYTES, fetch: post = fetch } = {}) {
  if (!url || !token) throw new Error("relayBundle needs the receiver's url and its upload token");
  if (request.method !== "POST") return refuse(405, "Send the bundle with POST");
  const type = (request.headers.get("content-type") || "").split(";")[0].trim().toLowerCase();
  if (type !== "application/zip") return refuse(415, "Send the bundle as application/zip");
  const length = request.headers.get("content-length");
  if (length === null || !/^\d+$/.test(length)) return refuse(411, "Send the bundle with a Content-Length");
  if (Number(length) > maxBytes) return refuse(413, `The bundle is over ${Math.floor(maxBytes / 1024 / 1024)} MB`);
  const body = await request.arrayBuffer();
  if (body.byteLength > maxBytes) return refuse(413, `The bundle is over ${Math.floor(maxBytes / 1024 / 1024)} MB`);
  const base = String(url).endsWith("/") ? String(url) : `${url}/`;
  const reply = await post(new URL("bundles", base), {
    method: "POST",
    headers: { "Content-Type": "application/zip", Authorization: `Bearer ${token}` },
    body,
  });
  return new Response(await reply.text(), {
    status: reply.status,
    headers: { "Content-Type": reply.headers.get("content-type") || "application/json" },
  });
}

function refuse(status, error) {
  return new Response(JSON.stringify({ ok: false, error }), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}
