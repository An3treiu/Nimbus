//! Serves the built frontend, embedded into the binary at compile time.
//!
//! In release builds the contents of `web/dist` are baked into the executable,
//! so Nimbus ships as a single self-contained binary. In debug builds the files
//! are read from disk, so frontend changes show up on refresh.

use axum::http::{header, StatusCode, Uri};
use axum::response::{Html, IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct WebAssets;

/// Serve an embedded asset by path, falling back to `index.html` (SPA).
pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = WebAssets::get(path) {
        let mime = file.metadata.mimetype().to_string();
        return ([(header::CONTENT_TYPE, mime)], file.data.into_owned()).into_response();
    }
    // Unknown path: serve the SPA entrypoint so client-side routing works.
    match WebAssets::get("index.html") {
        Some(file) => Html(file.data.into_owned()).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            "frontend not built — run `cd web && npm run build`",
        )
            .into_response(),
    }
}
