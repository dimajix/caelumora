//! The web UI, served from the binary (issue 323): what `npm run build`
//! made in `caelumora-web-ui/dist`, embedded in a release build and read
//! from there in a debug build, so that a new build of the UI needs no new
//! build of the program while it is developed.
//!
//! A path that names no file is answered with `index.html`, so that a
//! route of the UI's own works when it is loaded anew. `index.html` is never
//! cached - it names the files of the build it belongs to -, the files under
//! `assets/` are, for long: their names change with their content.

use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../caelumora-web-ui/dist"]
struct Build;

const INDEX: &str = "index.html";

/// Where the files whose names carry a hash of their content lie.
const HASHED: &str = "assets/";

/// The file `uri` names, or `index.html`.
pub(crate) async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    match Build::get(path).filter(|_| !path.is_empty()) {
        Some(file) => file_response(path, file.data.into_owned()),
        None => match Build::get(INDEX) {
            Some(index) => file_response(INDEX, index.data.into_owned()),
            // Only in a debug build whose UI was removed after the build.
            None => (StatusCode::NOT_FOUND, "The web UI is not built").into_response(),
        },
    }
}

fn file_response(path: &str, content: Vec<u8>) -> Response {
    let mut response = content.into_response();
    let headers = response.headers_mut();
    let media_type = mime_guess::from_path(path).first_or_octet_stream();
    if let Ok(value) = HeaderValue::from_str(media_type.as_ref()) {
        headers.insert(CONTENT_TYPE, value);
    }
    let caching = if path.starts_with(HASHED) { "public, max-age=31536000, immutable" } else { "no-cache" };
    headers.insert(CACHE_CONTROL, HeaderValue::from_static(caching));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn served(path: &str) -> (StatusCode, String, String) {
        let response = serve(path.parse().unwrap()).await;
        let header = |name| response.headers().get(name).map_or(String::new(), |v: &HeaderValue| v.to_str().unwrap().to_string());
        (response.status(), header(CONTENT_TYPE), header(CACHE_CONTROL))
    }

    #[tokio::test]
    async fn the_page_is_never_cached_and_a_hashed_file_for_long() {
        assert_eq!(served("/").await, (StatusCode::OK, "text/html".to_string(), "no-cache".to_string()));
        assert_eq!(served("/sessions/1").await.0, StatusCode::OK, "a route of the UI is the page");
        let hashed = Build::iter().find(|f| f.starts_with(HASHED) && f.ends_with(".js")).expect("the build has a script");
        let (status, media_type, caching) = served(&format!("/{hashed}")).await;
        assert_eq!((status, media_type.as_str()), (StatusCode::OK, "text/javascript"));
        assert!(caching.contains("immutable"), "{caching}");
    }
}
