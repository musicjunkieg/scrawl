// Sends a bundle's zip from the page. The receiver (scrawl-server) answers no CORS preflight and its
// token mustn't ship in a page, so the zip goes to the page's own backend, which passes it on with
// `relayBundle` (`scrawl-web/relay`).

/**
 * POSTs `zip` (a Blob) to `url` as `application/zip` and resolves with the reply's JSON; a refusal
 * throws, with the status and reply on the error.
 */
export async function sendZip(zip, { url, token, headers, signal } = {}) {
  if (!url) throw new Error("bundle.send needs a url: your backend's address that passes bundles on");
  const res = await fetch(url, {
    method: "POST",
    headers: {
      "Content-Type": "application/zip",
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
      ...headers,
    },
    body: zip,
    signal,
  });
  const text = await res.text();
  let reply;
  try {
    reply = text ? JSON.parse(text) : {};
  } catch {
    reply = { error: text.slice(0, 200) };
  }
  if (!res.ok) {
    const error = new Error(`The bundle wasn't sent (${res.status}): ${reply.error || res.statusText || "refused"}`);
    error.status = res.status;
    error.reply = reply;
    throw error;
  }
  return reply;
}
