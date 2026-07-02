//! Serve the built web SPA (`web/dist`) embedded in the binary via rust-embed,
//! with SPA fallback: unknown non-API paths return `index.html` so client-side
//! routing works on hard refresh.

use axum::body::Body;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct Assets;

/// Serve a static asset by path, falling back to `index.html` for unknown
/// routes (client-side SPA routing). Returns 404 only when even `index.html`
/// is absent (an unbuilt frontend).
pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match Assets::get(path) {
        Some(content) => serve(path, content),
        None => match Assets::get("index.html") {
            Some(content) => serve("index.html", content),
            None => (
                StatusCode::NOT_FOUND,
                "web UI not built (run `npm run build` in web/)",
            )
                .into_response(),
        },
    }
}

fn serve(path: &str, content: rust_embed::EmbeddedFile) -> Response {
    let mime = mime_for(path);
    let mut resp = Body::from(content.data.into_owned()).into_response();
    if let Ok(value) = header::HeaderValue::from_str(mime) {
        resp.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    // index.html must never be cached (it references hashed assets); hashed
    // assets are safe to cache aggressively.
    let cache = if path == "index.html" {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };
    if let Ok(value) = header::HeaderValue::from_str(cache) {
        resp.headers_mut().insert(header::CACHE_CONTROL, value);
    }
    resp
}

/// Minimal content-type lookup by extension for the asset kinds a Vite build
/// emits.
fn mime_for(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "map" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}
