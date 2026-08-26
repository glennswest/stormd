//! Serves the web UI — a Svelte SPA built into `web/dist` and embedded in
//! this binary at compile time. The Rust side is only a static file handler;
//! everything the UI shows comes over the same REST/WS API every other
//! client uses.

use axum::extract::{OriginalUri, Path};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};

#[derive(rust_embed::Embed)]
#[folder = "../../web/dist"]
struct Assets;

/// `/ui/` — the SPA entry point.
pub async fn index() -> Response {
    serve_asset("index.html")
}

/// `/ui/{*path}` — embedded assets, with the pre-SPA page URLs redirected to
/// their hash routes so old bookmarks and dashboard links keep working, and
/// anything unknown falling back to the SPA (which routes by hash anyway).
pub async fn asset(OriginalUri(uri): OriginalUri, Path(path): Path<String>) -> Response {
    match path.as_str() {
        "terminal" => return Redirect::permanent("/ui/#/terminal").into_response(),
        "logs" => {
            let q = uri.query().map(|q| format!("?{}", q)).unwrap_or_default();
            return Redirect::temporary(&format!("/ui/#/logs{}", q)).into_response();
        }
        _ => {}
    }
    if let Some(name) = path.strip_prefix("ext/") {
        return Redirect::permanent(&format!("/ui/#/ext/{}", name)).into_response();
    }
    serve_asset(&path)
}

fn serve_asset(path: &str) -> Response {
    match Assets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                [
                    (header::CONTENT_TYPE, mime.as_ref()),
                    // Filenames are fixed (no content hashes), so revalidate
                    // quickly rather than cache long.
                    (header::CACHE_CONTROL, "max-age=60"),
                ],
                file.data.into_owned(),
            )
                .into_response()
        }
        None => match Assets::get("index.html") {
            Some(file) => (
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                file.data.into_owned(),
            )
                .into_response(),
            None => (StatusCode::NOT_FOUND, "web UI not built into this binary").into_response(),
        },
    }
}
