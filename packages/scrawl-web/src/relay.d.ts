/** The receiver's default limit (`MAX_UPLOAD_BYTES`), zipped: 40 MB. */
export const MAX_UPLOAD_BYTES: number;

/**
 * For the page's backend: forwards the zip `bundle.send` posted to `<url>/bundles` on a
 * scrawl-server with its upload token, and answers with the receiver's reply. Check that the
 * person may send before calling it; this only forwards.
 */
export function relayBundle(
  request: Request,
  options: {
    /** The receiver's address, like `https://scrawl-server.you.workers.dev`. */
    url: string | URL;
    /** The receiver's upload token (`SCRAWL_UPLOAD_TOKEN`), kept on your server. */
    token: string;
    /** Largest zip passed on (40 MB). */
    maxBytes?: number;
    fetch?: typeof fetch;
  },
): Promise<Response>;
