//! `scrawl-server` on Cloudflare Workers. The receiver's logic is all in the `scrawl-server`
//! crate; this only turns a Workers request into its `Request`, keeps bundles in the R2 bucket
//! bound as `BUNDLES`, and reads the token from the `SCRAWL_TOKEN` secret. See `README.md` here.

use worker::{Bucket, Context, Date, Env, HttpMetadata, Method, Request, Response, Result, event};

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
async fn fetch(mut request: Request, env: Env, _ctx: Context) -> Result<Response> {
    let config = scrawl_server::Config {
        token: env
            .secret("SCRAWL_TOKEN")
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
    send(reply)
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
