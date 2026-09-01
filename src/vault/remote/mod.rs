//! Minimal WebDAV (RFC 4918) client used by the WebDAV Vault source.
//!
//! Scope is deliberately narrow: list a collection (PROPFIND depth-1),
//! fetch a file, PUT a file, DELETE a file, and MKCOL a directory. It uses
//! `reqwest` (already a transitive dependency via `hf-hub`, native-tls) and
//! `roxmltree` to parse the PROPFIND multistatus body. No heavyweight WebDAV
//! framework (ADR-06/13).
//!
//! Design note: this client is the remote-half of a WebDAV source. Exact-note
//! reads and atomic writes run on a LOCAL mirror checkout (the authoritative
//! Markdown path per ADR-01); this client only ever lists/gets/puts the mirror
//! during a sync turn. It must never be on the per-request note-read path.
//!
//! Marked `allow(dead_code)` until the WebDAV source (work packet
//! `docs/architecture/work-packet-webdav-vaultsource.md`, Phase C/D/E) wires
//! it into the runtime; the parser/encoding unit tests keep it live.

#![allow(dead_code)]

use std::time::Duration;

pub(crate) mod sync;
pub(crate) mod webdav_scheduler;

pub(crate) use webdav_scheduler::{WEBDAV_TICK_INTERVAL, WebDavScheduler, spawn_webdav_tick};

/// Credentials for a WebDAV endpoint. Kept out of debug output and projections.
#[derive(Clone)]
pub struct WebDavCredentials {
    pub username: String,
    /// Secret; `Debug` for the wrapper redacts this.
    pub password: String,
}

impl std::fmt::Debug for WebDavCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebDavCredentials")
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

/// A file entry returned by a PROPFIND listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebDavEntry {
    /// Path relative to the collection root, using `/` separators, no leading
    /// slash. Empty string is not produced for files.
    pub path: String,
    /// True if the entry is a collection (directory), false for a file.
    pub is_dir: bool,
    /// Resource size in bytes, when the server reported it (0 when unknown).
    pub size: u64,
    /// Optional ETag (opaque; used for optimistic-lock PUT comparisons).
    pub etag: Option<String>,
}

/// Client state: base URL + credentials + an HTTP client.
#[derive(Clone)]
pub struct WebDavClient {
    base: String,
    credentials: Option<WebDavCredentials>,
    http: reqwest::Client,
    timeout: Duration,
}

/// Errors from the WebDAV client. The message is a plain string so it can be
/// wrapped by the caller's existing error types.
#[derive(Debug)]
pub struct WebDavError(pub String);

impl std::fmt::Display for WebDavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WebDavError {}

impl From<reqwest::Error> for WebDavError {
    fn from(e: reqwest::Error) -> Self {
        WebDavError(format!("webdav http: {e}"))
    }
}

impl WebDavClient {
    pub fn new(base_url: &str, credentials: Option<WebDavCredentials>) -> Result<Self, WebDavError> {
        let base = base_url.trim().trim_end_matches('/').to_string();
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err(WebDavError("WebDAV URL must be http(s)://".to_string()));
        }
        // Per-request timeouts; a hung remote must never hold a sync turn or a
        // read indefinitely.
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| WebDavError(format!("webdav client build: {e}")))?;
        Ok(Self {
            base,
            credentials,
            http,
            timeout: Duration::from_secs(60),
        })
    }

    fn auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.credentials {
            Some(cred) => request.basic_auth(&cred.username, Some(&cred.password)),
            None => request,
        }
    }

    /// Join a relative path (already `/`-separated) onto the base, URL-encoding
    /// each segment so filenames with spaces/`?`/`#`/`&` survive. No trailing
    /// slash is appended, so this addresses a FILE resource (GET/PUT/DELETE).
    /// Use [`Self::collection_url`] for PROPFIND/MKCOL on a collection.
    fn file_url(&self, relative: &str) -> String {
        let mut url = self.base.clone();
        if !url.ends_with('/') {
            url.push('/');
        }
        let segments: Vec<_> = relative.split('/').filter(|s| !s.is_empty()).collect();
        for (i, segment) in segments.iter().enumerate() {
            url.push_str(&urlencode_segment(segment));
            if i + 1 < segments.len() {
                url.push('/');
            }
        }
        url
    }

    /// Join a relative collection path with a trailing slash (PROPFIND, MKCOL).
    fn collection_url(&self, relative: &str) -> String {
        let mut url = self.file_url(relative);
        if !url.ends_with('/') {
            url.push('/');
        }
        url
    }

    /// List the entries directly under `collection_relative` (depth-1).
    /// Returns entries relative to the collection root (no leading slash).
    pub async fn list(&self, collection_relative: &str) -> Result<Vec<WebDavEntry>, WebDavError> {
        let href = self.collection_url(collection_relative);
        let resp = self
            .auth(
                self.http
                    .request(webdav_method("PROPFIND"), &href)
                    .header("Depth", "1")
                    .header("Content-Type", "application/xml"),
            )
            .body(
                r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:">
  <d:prop>
    <d:resourcetype/>
    <d:getcontentlength/>
    <d:getetag/>
  </d:prop>
</d:propfind>"#,
            )
            .send()
            .await
            .map_err(WebDavError::from)?
            .error_for_status()
            .map_err(|e| {
                WebDavError(format!(
                    "webdav PROPFIND {collection_relative}: {e}"
                ))
            })?;
        let body = resp.text().await.map_err(WebDavError::from)?;
        parse_multistatus(&body)
    }

    /// Fetch a file's bytes.
    pub async fn get(&self, relative: &str) -> Result<Vec<u8>, WebDavError> {
        let href = self.file_url(relative);
        let resp = self
            .auth(self.http.request(reqwest::Method::GET, &href))
            .send()
            .await
            .map_err(WebDavError::from)?
            .error_for_status()
            .map_err(|e| WebDavError(format!("webdav GET {relative}: {e}")))?;
        Ok(resp.bytes().await.map_err(WebDavError::from)?.to_vec())
    }

    /// Create or overwrite a file. If `expected_etag` is `Some`, uses
    /// `If-Match` (optimistic lock); a 412 means the remote changed.
    pub async fn put(
        &self,
        relative: &str,
        contents: &[u8],
        expected_etag: Option<&str>,
    ) -> Result<(), WebDavError> {
        let href = self.file_url(relative);
        let mut builder = self
            .auth(self.http.request(reqwest::Method::PUT, &href))
            .header("Content-Type", "text/markdown; charset=utf-8")
            .body(contents.to_vec());
        if let Some(etag) = expected_etag {
            builder = builder.header("If-Match", etag);
        }
        builder
            .send()
            .await
            .map_err(WebDavError::from)?
            .error_for_status()
            .map_err(|e| WebDavError(format!("webdav PUT {relative}: {e}")))?;
        Ok(())
    }

    /// Create a directory collection.
    pub async fn mkdir(&self, relative: &str) -> Result<(), WebDavError> {
        let href = self.collection_url(relative);
        self.auth(self.http.request(webdav_method("MKCOL"), &href))
            .send()
            .await
            .map_err(WebDavError::from)?
            .error_for_status()
            .map_err(|e| WebDavError(format!("webdav MKCOL {relative}: {e}")))?;
        Ok(())
    }

    /// Delete a file or (unused for dirs by callers) collection.
    pub async fn delete(&self, relative: &str) -> Result<(), WebDavError> {
        let href = self.file_url(relative);
        self.auth(self.http.request(reqwest::Method::DELETE, &href))
            .send()
            .await
            .map_err(WebDavError::from)?
            .error_for_status()
            .map_err(|e| WebDavError(format!("webdav DELETE {relative}: {e}")))?;
        Ok(())
    }

    #[cfg(test)]
    fn timeout(&self) -> Duration {
        self.timeout
    }
}

/// Percent-encode a single path segment (reserving `/` and unreserved chars).
fn urlencode_segment(segment: &str) -> String {
    const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut out = String::with_capacity(segment.len());
    for &byte in segment.as_bytes() {
        if UNRESERVED.contains(&byte) {
            out.push(byte as char);
        } else {
            for b in [b'%', HEX[byte as usize >> 4], HEX[byte as usize & 0xf]] {
                out.push(b as char);
            }
        }
    }
    out
}

/// Build a reqwest `Method` for a WebDAV verb not in the standard set
/// (`PROPFIND`, `MKCOL`). Infallible for our fixed strings.
fn webdav_method(name: &str) -> reqwest::Method {
    reqwest::Method::from_bytes(name.as_bytes())
        .expect("valid HTTP method token for WebDAV verb")
}

/// Percent-decode a URI path (RFC 3986): `%XX` → byte, `+` left as-is (it is
/// only a space in query strings, not paths).
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = hex_val(bytes[i + 1]);
                let lo = hex_val(bytes[i + 2]);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push(hi * 16 + lo);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

const HEX: [u8; 16] = *b"0123456789ABCDEF";

/// Parse a PROPFIND multistatus XML body into entries. Entries are made
/// relative to the collection root by stripping the outermost collection's
/// href from every child href.
fn parse_multistatus(xml: &str) -> Result<Vec<WebDavEntry>, WebDavError> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| WebDavError(format!("webdav PROPFIND parse: {e}")))?;
    // Find the outermost href used as the collection root.
    let mut root_href: Option<String> = None;
    let mut entries = Vec::new();
    for response in doc.descendants().filter(|n| {
        n.has_tag_name("response") && n.tag_name().namespace() == Some("DAV:")
    }) {
        let mut href: Option<String> = None;
        let mut is_dir = false;
        let mut size: u64 = 0;
        let mut etag: Option<String> = None;
        for node in response.children().filter(|n| n.is_element()) {
            match node.tag_name().name() {
                "href" => {
                    href = node.text().map(str::trim).map(str::to_string);
                }
                "propstat" => {
                    for prop in node.descendants() {
                        match prop.tag_name().name() {
                            "resourcetype" => {
                                is_dir = prop
                                    .children()
                                    .any(|c| c.is_element() && c.has_tag_name("collection"));
                            }
                            "getcontentlength" => {
                                size = prop
                                    .text()
                                    .and_then(|t| t.parse::<u64>().ok())
                                    .unwrap_or(0);
                            }
                            "getetag" => {
                                etag = prop.text().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        let Some(href) = href else { continue; };
        // The multistatus root is the collection we queried; its entry is
        // path "". Child paths are root + "/" + relative.
        if root_href.is_none() {
            root_href = Some(href.clone());
            entries.push(WebDavEntry { path: String::new(), is_dir: true, size, etag });
            continue;
        }
        let relative = href_strip_prefix(&href, root_href.as_deref().unwrap_or(""));
        // PROPFIND hrefs are URI-encoded (e.g. spaces as %20); decode the path.
        let path = percent_decode(&relative.trim_end_matches('/'));
        entries.push(WebDavEntry { path, is_dir, size, etag });
    }
    Ok(entries)
}

/// Strip `href` down to a path relative to `root`. Both are URI hrefs.
fn href_strip_prefix(href: &str, root: &str) -> String {
    let mut h = href;
    // lose scheme+authority if present, so we compare paths.
    if let Some(idx) = h.find("://") {
        if let Some(rest) = h[idx + 3..].find('/') {
            h = &h[idx + 3 + rest..];
        }
    }
    let mut r = root;
    if let Some(idx) = r.find("://") {
        if let Some(rest) = r[idx + 3..].find('/') {
            r = &r[idx + 3 + rest..];
        }
    }
    let h = h.trim_start_matches('/');
    let r = r.trim_start_matches('/');
    let h = h.strip_prefix(r).unwrap_or(h).trim_start_matches('/');
    h.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_encodes_segments() {
        assert_eq!(urlencode_segment("Home.md"), "Home.md");
        assert_eq!(urlencode_segment("a b"), "a%20b");
        assert_eq!(urlencode_segment("a?b"), "a%3Fb");
        assert_eq!(urlencode_segment("a#b"), "a%23b");
        assert_eq!(urlencode_segment("a&b"), "a%26b");
    }

    #[test]
    fn url_for_joins_and_encodes() {
        let c = WebDavClient::new("https://ex/dav", None).unwrap();
        // file URLs have no trailing slash; collections do.
        assert_eq!(c.file_url(""), "https://ex/dav/");
        assert_eq!(c.collection_url(""), "https://ex/dav/");
        assert_eq!(c.file_url("Home.md"), "https://ex/dav/Home.md");
        assert_eq!(c.file_url("Notes/a b.md"), "https://ex/dav/Notes/a%20b.md");
        assert_eq!(c.collection_url("Notes"), "https://ex/dav/Notes/");
    }

    #[test]
    fn parses_multistatus() {
        let xml = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/dav/</D:href>
    <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop>
    <D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/Home.md</D:href>
    <D:propstat><D:prop>
      <D:resourcetype/>
      <D:getcontentlength>123</D:getcontentlength>
      <D:getetag>"abc"</D:getetag>
    </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/Notes/</D:href>
    <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop>
    <D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/Notes/Note%20One.md</D:href>
    <D:propstat><D:prop>
      <D:resourcetype/>
      <D:getcontentlength>20</D:getcontentlength>
    </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
</D:multistatus>"#;
        let entries = parse_multistatus(xml).unwrap();
        assert_eq!(entries.len(), 4);
        assert!(entries[0].is_dir);
        assert_eq!(entries[0].path, "");
        // file
        assert!(!entries[1].is_dir);
        assert_eq!(entries[1].path, "Home.md");
        assert_eq!(entries[1].size, 123);
        assert_eq!(entries[1].etag.as_deref(), Some("\"abc\""));
        // subdir (trailing slash stripped)
        assert!(entries[2].is_dir);
        assert_eq!(entries[2].path, "Notes");
        // percent-encoded space in a nested file decoded
        assert!(!entries[3].is_dir);
        assert_eq!(entries[3].path, "Notes/Note One.md");
    }

    #[test]
    fn percent_decode_handles_encodings() {
        assert_eq!(percent_decode("Home.md"), "Home.md");
        assert_eq!(percent_decode("Note%20One.md"), "Note One.md");
        assert_eq!(percent_decode("a%2Fb"), "a/b");
        assert_eq!(percent_decode("100%25"), "100%");
        assert_eq!(percent_decode("caf%C3%A9"), "café");
        assert_eq!(percent_decode("a+b"), "a+b"); // `+` is literal in a path
        assert_eq!(percent_decode("a%2"), "a%2"); // malformed escape stays literal
    }

    #[test]
    fn credentials_redact_password_in_debug() {
        let c = WebDavCredentials { username: "u".to_string(), password: "s3cret".to_string() };
        let s = format!("{c:?}");
        assert!(!s.contains("s3cret"));
        assert!(s.contains("[REDACTED]"));
    }
}