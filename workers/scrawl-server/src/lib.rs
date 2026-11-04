//! `scrawl-server` on Cloudflare Workers. The receiver's logic is all in the `scrawl-server`
//! crate; this only turns a Workers request into its `Request`, keeps bundles in the R2 bucket
//! bound as `BUNDLES`, and reads the token from the `SCRAWL_TOKEN` secret. See `README.md` here.

use std::time::Duration;

use worker::{
    Bucket, Context, Date, Delay, Env, Fetch, Headers, HttpMetadata, Method, Request, RequestInit,
    Response, Result, event,
};

struct R2(Bucket);

impl scrawl_server::Store for R2 {
    async fn get(&self, key: &str) -> std::result::Result<Option<Vec<u8>>, String> {
        let Some(object) = self.0.get(key).execute().await.map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        match object.body() {
            Some(body) => body.bytes().await.map(Some).map_err(|e| e.to_string()),
            None => Ok(Some(Vec::new())),
        }
    }

    async fn put(
        &self,
        key: &str,
        bytes: Vec<u8>,
        content_type: &str,
    ) -> std::result::Result<(), String> {
        self.0
            .put(key, bytes)
            .http_metadata(HttpMetadata {
                content_type: Some(content_type.to_owned()),
                ..Default::default()
            })
            .execute()
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn list(&self, prefix: &str, limit: usize) -> std::result::Result<Vec<String>, String> {
        // R2 can answer with fewer keys than asked for and a cursor for the rest.
        let mut keys = Vec::new();
        let mut cursor: Option<String> = None;
        while keys.len() < limit {
            let mut listing = self
                .0
                .list()
                .prefix(prefix)
                .limit((limit - keys.len()).min(1000) as u32);
            if let Some(cursor) = cursor.take() {
                listing = listing.cursor(cursor);
            }
            let page = listing.execute().await.map_err(|e| e.to_string())?;
            keys.extend(page.objects().iter().map(|o| o.key()));
            match page.cursor().filter(|_| page.truncated()) {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        keys.truncate(limit);
        Ok(keys)
    }
}

#[event(fetch)]
async fn fetch(mut request: Request, env: Env, ctx: Context) -> Result<Response> {
    let config = scrawl_server::Config {
        token: env
            .secret("SCRAWL_TOKEN")
            .map(|s| s.to_string())
            .unwrap_or_default(),
        upload_token: env
            .secret("SCRAWL_UPLOAD_TOKEN")
            .map(|s| s.to_string())
            .unwrap_or_default(),
        max_upload: env
            .var("MAX_UPLOAD_BYTES")
            .ok()
            .and_then(|v| v.to_string().parse().ok())
            .unwrap_or(scrawl_server::DEFAULT_MAX_UPLOAD),
    };
    let url = request.url()?;
    let path = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_owned(),
    };
    let method = request.method();
    let authorization = scrawl_server::authorization(
        &config,
        request.headers().get("Authorization")?,
        request.headers().get("X-Scrawl-Token")?,
    );
    // A wrong token or an upload that says it's too big is turned away before its body is read.
    let length = request
        .headers()
        .get("Content-Length")?
        .and_then(|n| n.parse().ok());
    if let Some(reply) = scrawl_server::answer_before_body(
        &config,
        method.as_ref(),
        &path,
        authorization.as_deref(),
        length,
    ) {
        return send(reply);
    }
    // The upload token, which an app carries, sends at most 20 bundles a minute from one address
    // (`UPLOADS` in cloudflare.config.ts). Without the binding (another runtime), no limit.
    if scrawl_server::counts_toward_upload_limit(
        &config,
        method.as_ref(),
        &path,
        authorization.as_deref(),
    ) {
        if let Ok(limiter) = env.rate_limiter("UPLOADS") {
            let from = request
                .headers()
                .get("CF-Connecting-IP")?
                .unwrap_or_default();
            if !limiter.limit(from).await?.success {
                return send(scrawl_server::too_many_uploads());
            }
        }
    }
    let body = if method == Method::Post {
        request.bytes().await?
    } else {
        Vec::new()
    };
    let reply = scrawl_server::handle(
        &config,
        scrawl_server::Request {
            method: method.to_string(),
            path,
            authorization,
            body,
            now_ms: Date::now().as_millis(),
        },
        &R2(env.bucket("BUNDLES")?),
    )
    .await;
    // A new bundle rings the webhook, if one is set, once the uploader has its answer.
    if let Some(entry) = reply.saved.clone()
        && let Ok(hook) = env.secret("SCRAWL_WEBHOOK_URL")
    {
        let base = url.origin().ascii_serialization();
        let secret = env
            .secret("SCRAWL_WEBHOOK_SECRET")
            .map(|s| s.to_string())
            .unwrap_or_default();
        let bucket = env.bucket("BUNDLES")?;
        ctx.wait_until(async move {
            ring(hook.to_string(), secret, *entry, base, R2(bucket)).await;
        });
    }
    send(reply)
}

/// Sends the webhook for `entry`, trying three times (now, after 2 s, after 8 s): a Worker may
/// keep working for 30 s after it answers. What still fails is logged; the bundle is kept either
/// way, and the courier and MCP read it from the list.
async fn ring(hook: String, secret: String, entry: scrawl_server::Entry, base: String, store: R2) {
    if !scrawl_server::webhook::allowed(&hook) {
        worker::console_error!("scrawl-server: SCRAWL_WEBHOOK_URL must be an https:// address");
        return;
    }
    let body = match scrawl_server::webhook::message(&entry, &base, &store).await {
        Ok(body) => body,
        Err(error) => {
            worker::console_error!("scrawl-server: webhook: storage: {error}");
            return;
        }
    };
    let mut last = String::new();
    for wait in [0, 2, 8] {
        if wait > 0 {
            Delay::from(Duration::from_secs(wait)).await;
        }
        match post(&hook, &secret, &entry.id, &body).await {
            Ok(status) if (200..300).contains(&status) => return,
            Ok(status) => last = format!("answered {status}"),
            Err(error) => last = error.to_string(),
        }
    }
    worker::console_error!(
        "scrawl-server: webhook for {} not delivered: {last}",
        entry.id
    );
}

async fn post(hook: &str, secret: &str, id: &str, body: &[u8]) -> Result<u16> {
    let headers = Headers::new();
    headers.set("Content-Type", "application/json")?;
    headers.set("User-Agent", "scrawl-server")?;
    headers.set("X-Scrawl-Event", scrawl_server::webhook::EVENT)?;
    headers.set("X-Scrawl-Delivery", id)?;
    // Signed when a secret is set; a chat app's address (which is itself the secret) needs none.
    if !secret.is_empty() {
        let now = Date::now().as_millis() / 1000;
        headers.set("X-Scrawl-Timestamp", &now.to_string())?;
        headers.set(
            "X-Scrawl-Signature",
            &scrawl_server::webhook::signature(secret, now, body),
        )?;
    }
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(body.to_vec().into()));
    let response = Fetch::Request(Request::new_with_init(hook, &init)?)
        .send()
        .await?;
    Ok(response.status_code())
}

fn send(reply: scrawl_server::Response) -> Result<Response> {
    if let Some(line) = &reply.log {
        worker::console_error!("scrawl-server: {line}");
    }
    let mut response = Response::from_bytes(reply.body)?.with_status(reply.status);
    response
        .headers_mut()
        .set("Content-Type", reply.content_type)?;
    Ok(response)
}
