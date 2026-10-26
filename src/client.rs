//! The GraphQL client and the API key it sends.
//!
//! linearctl speaks GraphQL to api.linear.app directly (no SDK); every
//! document it sends is in queries.rs. One agent per process, so a
//! command's requests, and serve's, share one connection.

use crate::fail::{self, Fail, R, failed};
use crate::json;
use crate::run::{Run, VERSION};
use crate::util::printable;
use serde_json::{Map, Value, json};
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, mpsc};
use std::time::Duration;
use ureq::http::HeaderMap;

/// The API and the file host. LINEARCTL_API_URL and LINEARCTL_UPLOADS_URL
/// replace them for tests; nothing else should set them, since the API key
/// goes wherever they point.
const API_URL: &str = "https://api.linear.app/graphql";
const UPLOADS_URL: &str = "https://uploads.linear.app";

/// A file download's cap; Linear's own upload limit is lower.
pub const MAX_DOWNLOAD: u64 = 256 << 20;

pub fn endpoint() -> String {
    std::env::var("LINEARCTL_API_URL").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| API_URL.into())
}

pub fn uploads_origin() -> String {
    match std::env::var("LINEARCTL_UPLOADS_URL") {
        Ok(u) if !u.is_empty() => u.trim_end_matches('/').to_string(),
        _ => UPLOADS_URL.into(),
    }
}

/// The host (and port) of a URL, as errors name it.
pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?', '#']).next().unwrap_or("").rsplit('@').next().unwrap_or("").to_string()
}

/// Linear's answer: data (null when absent) and its errors.
pub struct Answer {
    pub data: Value,
    pub errors: Vec<Value>,
}

pub struct Client {
    pub key: String,
    /// "env", "keychain", "file", "stdin" or "demo".
    pub source: String,
    /// The GraphQL endpoint: endpoint(), or a test's server.
    url: String,
    timeout: u64,
    demo: bool,
    /// Seconds file URLs in answers stay signed for; 0 leaves them as they are.
    pub signed_urls: Cell<u64>,
    /// The last response's headers, for --meta and rate limits.
    header: RefCell<Option<HeaderMap>>,
    requests: Cell<u64>,
    /// Whether the 'waiting on' note was said: once per process, but a test
    /// has its own.
    noted: &'static AtomicBool,
}

/// Said once per process when a request takes more than 2 s.
static WAIT_NOTE: AtomicBool = AtomicBool::new(false);

/// The process's connections to Linear: TLS with the platform's
/// certificates, and the proxy the environment names.
fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .user_agent(format!("linearctl/{VERSION}"))
            .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
            .build()
            .into()
    })
}

impl Client {
    /// Finds the API key; without one it is an auth error (exit 5).
    pub fn new(run: &Run) -> R<Client> {
        if run.demo {
            return Ok(Client::with_key(run, "demo", "demo"));
        }
        let (key, source) = crate::keys::api_key()?;
        if key.is_empty() {
            return Err(Fail::Auth(
                "no API key: set LINEAR_API_KEY, or pipe one to 'linearctl auth login --yes'".into(),
            ));
        }
        Ok(Client::with_key(run, &key, &source))
    }

    pub fn with_key(run: &Run, key: &str, source: &str) -> Client {
        Client {
            key: key.into(),
            source: source.into(),
            url: endpoint(),
            timeout: run.timeout,
            demo: run.demo,
            signed_urls: Cell::new(0),
            header: RefCell::new(None),
            requests: Cell::new(0),
            noted: &WAIT_NOTE,
        }
    }

    /// --timeout for a whole request; 0 is none, as it was.
    fn limit(&self) -> Option<Duration> {
        (self.timeout > 0).then(|| Duration::from_secs(self.timeout))
    }

    pub fn has_header(&self) -> bool {
        self.header.borrow().is_some()
    }

    fn header(&self, name: &str) -> String {
        self.header.borrow().as_ref().and_then(|h| h.get(name)).and_then(|v| v.to_str().ok()).unwrap_or("").to_string()
    }

    /// Sends doc and returns its data. Any GraphQL error fails it.
    pub fn query(&self, run: &mut Run, doc: &str, vars: Value) -> R<Value> {
        let a = self.raw(run, doc, vars)?;
        if !a.errors.is_empty() {
            return Err(linear_failure(&a.errors));
        }
        Ok(a.data)
    }

    /// Sends doc and returns Linear's answer, errors included. Transport,
    /// auth and rate-limit failures are errors; GraphQL errors are left to
    /// the caller.
    pub fn raw(&self, run: &mut Run, doc: &str, vars: Value) -> R<Answer> {
        let mut req = Map::new();
        req.insert("query".into(), json!(doc));
        req.insert("variables".into(), vars);
        self.send(run, Value::Object(req))
    }

    /// Posts a whole GraphQL request body ({query, variables, operationName}).
    pub fn send(&self, run: &mut Run, request: Value) -> R<Answer> {
        if self.demo {
            self.requests.set(self.requests.get() + 1);
            let doc = request["query"].as_str().unwrap_or("").to_string();
            return Ok(crate::demo::answer(&doc, &request["variables"]));
        }
        let body = json::marshal(&request);
        let url = self.url.clone();
        let mut req = agent()
            .post(&url)
            .config()
            .timeout_global(self.limit())
            .build()
            .header("Authorization", &self.key)
            .header("Content-Type", "application/json");
        if self.signed_urls.get() > 0 {
            req = req.header("public-file-urls-expire-in", &self.signed_urls.get().to_string());
        }
        self.requests.set(self.requests.get() + 1);
        let (status, headers, data) = self.wait(run, &url, move || {
            let mut res = req.send(body.as_bytes())?;
            let status = res.status().as_u16();
            let headers = res.headers().clone();
            let data = res.body_mut().with_config().limit(64 << 20).read_to_vec();
            Ok((status, headers, data))
        })?;
        *self.header.borrow_mut() = Some(headers);
        let data =
            data.map_err(|e| failed(format!("reading Linear's answer: {}", transport_cause(&e))).hint(fail::RERUN))?;
        let parsed = json::parse(&data);
        let errors: Vec<Value> = match &parsed {
            Ok(v) => v.get("errors").and_then(Value::as_array).cloned().unwrap_or_default(),
            Err(_) => vec![],
        };
        for e in &errors {
            match e["extensions"]["code"].as_str() {
                Some("AUTHENTICATION_ERROR") => return Err(rejected()),
                Some("RATELIMITED") => return Err(self.limited()),
                _ => {}
            }
        }
        // Go decoded the answer into {data, errors}: anything else is not JSON
        // it understood.
        let wellformed = match &parsed {
            Ok(Value::Null) => true,
            Ok(Value::Object(m)) => m.get("errors").is_none_or(|e| {
                e.is_null() || e.as_array().is_some_and(|a| a.iter().all(|x| x.is_object() || x.is_null()))
            }),
            _ => false,
        };
        match status {
            401 => return Err(rejected()),
            429 => return Err(self.limited()),
            _ if !wellformed => {
                return Err(server(status, failed(format!("Linear answered HTTP {status} without JSON"))));
            }
            s if s >= 400 && errors.is_empty() => return Err(server(s, failed(format!("Linear answered HTTP {s}")))),
            _ => {}
        }
        let mut v = parsed.unwrap_or(Value::Null);
        Ok(Answer { data: v.get_mut("data").map(Value::take).unwrap_or(Value::Null), errors })
    }

    /// Runs a request off this thread and says on stderr what it is waiting
    /// on once it takes more than 2 s.
    fn wait<T: Send + 'static>(
        &self,
        run: &mut Run,
        url: &str,
        f: impl FnOnce() -> Result<T, ureq::Error> + Send + 'static,
    ) -> R<T> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        let host = host_of(url);
        let result = match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(r) => r,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !self.noted.swap(true, Ordering::Relaxed) {
                    run.eprint(&format!("linearctl: waiting on {host}\n"));
                }
                rx.recv().map_err(|_| failed(format!("cannot reach {host}: the request was lost")).hint(fail::RERUN))?
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(failed(format!("cannot reach {host}")).hint(fail::RERUN));
            }
        };
        result.map_err(|e| {
            match e {
                ureq::Error::Timeout(_) => {
                    failed(format!("{host} did not answer within {} s; raise --timeout", self.timeout))
                }
                e => failed(format!("cannot reach {host}: {}", transport_cause(&e))),
            }
            .hint(fail::RERUN)
        })
    }

    /// A rate-limit error with the latest reset among the budgets that are
    /// spent, from Linear's X-RateLimit-* headers.
    fn limited(&self) -> Fail {
        let mut at: Option<i64> = None;
        for kind in ["Requests", "Complexity", "Endpoint-Requests"] {
            if self.header(&format!("X-RateLimit-{kind}-Remaining")) != "0" {
                continue;
            }
            if let Ok(ms) = self.header(&format!("X-RateLimit-{kind}-Reset")).parse::<i64>()
                && at.is_none_or(|a| ms > a)
            {
                at = Some(ms);
            }
        }
        let mut msg = "Linear's rate limit is spent".to_string();
        if let Some(ms) = at {
            msg.push_str(&format!("; retry after {}", crate::time::rfc3339(ms * 1_000_000)));
        }
        Fail::Limited(msg, at)
    }

    /// What --meta adds to a result: the requests this run sent, and the
    /// budget left after the last one.
    pub fn meta(&self) -> Value {
        let complexity = self.header("X-Complexity").parse::<f64>().ok().map_or(Value::Null, |n| json!(n));
        json!({"requests": self.requests.get(), "rateLimit": self.rate_limit(), "complexity": complexity})
    }

    /// The budget left after the last request, from its headers.
    pub fn rate_limit(&self) -> Value {
        let mut out = Map::new();
        for kind in ["Requests", "Complexity"] {
            let Ok(n) = self.header(&format!("X-RateLimit-{kind}-Remaining")).parse::<i64>() else { continue };
            let mut entry = Map::new();
            entry.insert("remaining".into(), json!(n));
            if let Ok(limit) = self.header(&format!("X-RateLimit-{kind}-Limit")).parse::<i64>() {
                entry.insert("limit".into(), json!(limit));
            }
            if let Ok(ms) = self.header(&format!("X-RateLimit-{kind}-Reset")).parse::<i64>() {
                entry.insert("resetAt".into(), json!(crate::time::rfc3339(ms * 1_000_000)));
            }
            out.insert(kind.to_lowercase(), Value::Object(entry));
        }
        Value::Object(out)
    }

    /// The query's complexity from the last answer, if Linear said.
    pub fn complexity(&self) -> Option<f64> {
        self.header("X-Complexity").parse().ok()
    }

    /// When file URLs signed in the next request stop working: Linear signs
    /// them from when it answers, so this can only be early, never late.
    pub fn signed_until(&self) -> Value {
        match self.signed_urls.get() {
            0 => Value::Null,
            s => json!(crate::time::rfc3339(crate::time::now() + s as i64 * crate::time::SECOND)),
        }
    }

    /// Fetches a file with the API key: its bytes and content type.
    pub fn download(&self, run: &mut Run, url: &str) -> R<(Vec<u8>, String)> {
        if self.demo {
            return Err(failed(format!("demo mode has no files: {url}")));
        }
        let req = agent().get(url).config().timeout_global(self.limit()).build().header("Authorization", &self.key);
        let (status, kind, data) = self.wait(run, url, move || {
            let mut res = req.call()?;
            let status = res.status().as_u16();
            let kind = res.headers().get("Content-Type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            let data = if status == 200 {
                Some(res.body_mut().with_config().limit(MAX_DOWNLOAD + 1).read_to_vec())
            } else {
                None
            };
            Ok((status, kind, data))
        })?;
        match status {
            401 => return Err(rejected()),
            404 | 403 => return Err(failed(format!("not found: {url} (or this API key cannot see it)"))),
            200 => {}
            s => return Err(server(s, failed(format!("{} answered HTTP {s}", host_of(url))))),
        }
        let data = match data {
            Some(Ok(d)) => d,
            Some(Err(ureq::Error::BodyExceedsLimit(_))) => {
                return Err(failed(format!("{url} is larger than {} MiB", MAX_DOWNLOAD >> 20)));
            }
            Some(Err(e)) => return Err(failed(format!("reading {url}: {}", transport_cause(&e)))),
            None => vec![],
        };
        if data.len() as u64 > MAX_DOWNLOAD {
            return Err(failed(format!("{url} is larger than {} MiB", MAX_DOWNLOAD >> 20)));
        }
        Ok((data, kind))
    }
}

/// A failed answer; Linear's own trouble (5xx) may pass, so a rerun is worth
/// it, and safe.
fn server(status: u16, f: Fail) -> Fail {
    if status >= 500 { f.hint(fail::RERUN) } else { f }
}

fn rejected() -> Fail {
    Fail::Auth("Linear rejected the API key; pipe a new one to 'linearctl auth login --yes'".into())
}

/// A transport error in the words of the OS where there are some.
fn transport_cause(e: &ureq::Error) -> String {
    match e {
        ureq::Error::Io(io) => crate::state::cause(io),
        e => e.to_string(),
    }
}

/// Linear's first error, in the words it offers people.
pub fn linear_failure(errors: &[Value]) -> Fail {
    let e = &errors[0];
    let message = e["message"].as_str().unwrap_or("");
    let msg = match e["extensions"]["userPresentableMessage"].as_str() {
        Some(m) if !m.is_empty() => m,
        _ => message,
    };
    Fail::Linear { msg: printable(msg), not_found: message.to_lowercase().contains("not found") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::Sink;
    use crate::testserver::{Response, serve};
    use std::sync::atomic::Ordering;

    const KEY: &str = "lin_api_test0000000000000000";

    fn run() -> Run {
        Run::new(Sink::Buf(vec![]), Sink::Buf(vec![]))
    }

    fn client(run: &Run, url: &str) -> Client {
        let mut c = Client::with_key(run, KEY, "env");
        c.url = url.to_string();
        c
    }

    /// Answers like Linear: the key checked, then a fixed answer.
    fn linear(answer: impl Fn() -> Response + Send + Sync + 'static) -> crate::testserver::Server {
        serve(move |req| {
            if req.header("Authorization") != Some(KEY) {
                return Response::json(
                    400,
                    r#"{"errors":[{"message":"Authentication required","extensions":{"code":"AUTHENTICATION_ERROR"}}]}"#,
                );
            }
            answer()
        })
    }

    #[test]
    fn sends_the_key_and_decodes() {
        let api = linear(|| Response::json(200, r#"{"data":{"viewer":{"id":"u1","name":"Ada"}}}"#));
        let mut r = run();
        let data = client(&r, &api.url).query(&mut r, crate::queries::VIEWER, Value::Null).unwrap();
        assert_eq!(data["viewer"]["name"], "Ada");
        let reqs = api.requests.lock().unwrap();
        let h = &reqs[0];
        assert_eq!(h.header("Authorization"), Some(KEY));
        assert_eq!(h.header("public-file-urls-expire-in"), None);
        assert!(h.header("User-Agent").unwrap().starts_with("linearctl/"));
        assert_eq!(h.header("Content-Type"), Some("application/json"));
        let body: Value = serde_json::from_str(&h.body).unwrap();
        assert_eq!(body["query"], crate::queries::VIEWER);
        assert!(body["variables"].is_null());
    }

    #[test]
    fn signs_file_urls_when_asked() {
        let api = linear(|| Response::json(200, r#"{"data":{}}"#));
        let mut r = run();
        let c = client(&r, &api.url);
        c.signed_urls.set(300);
        let until = c.signed_until();
        c.query(&mut r, crate::queries::VIEWER, Value::Null).unwrap();
        assert_eq!(api.requests.lock().unwrap()[0].header("public-file-urls-expire-in"), Some("300"));
        assert!(until.as_str().unwrap().ends_with('Z'));
    }

    #[test]
    fn maps_failures() {
        let reset = (crate::time::now() / 1_000_000 + 600_000).to_string();
        let mut r = run();
        // A key Linear rejects.
        let api = linear(|| Response::json(200, "{}"));
        let mut c = client(&r, &api.url);
        c.key = "lin_api_wrong000000000000000".into();
        assert!(matches!(c.query(&mut r, crate::queries::VIEWER, Value::Null), Err(Fail::Auth(_))));
        // Rate limited, with when it resets.
        let at = reset.clone();
        let api = linear(move || Response {
            status: 400,
            headers: vec![
                ("X-RateLimit-Requests-Remaining".into(), "0".into()),
                ("X-RateLimit-Requests-Reset".into(), at.clone()),
            ],
            body: r#"{"errors":[{"message":"Rate limit exceeded","extensions":{"code":"RATELIMITED"}}]}"#.into(),
        });
        match client(&r, &api.url).query(&mut r, crate::queries::VIEWER, Value::Null) {
            Err(Fail::Limited(msg, Some(ms))) => {
                assert_eq!(ms.to_string(), reset);
                assert!(msg.contains("retry after"), "{msg}");
            }
            other => panic!("{other:?}"),
        }
        let api = linear(|| Response::json(429, "{}"));
        assert!(matches!(
            client(&r, &api.url).query(&mut r, crate::queries::VIEWER, Value::Null),
            Err(Fail::Limited(..))
        ));
        // Linear's own error, and not found named.
        let api = linear(|| {
            Response::json(
                400,
                r#"{"errors":[{"message":"Entity not found: Issue","extensions":{"code":"INVALID_INPUT"}}]}"#,
            )
        });
        let e = client(&r, &api.url).query(&mut r, crate::queries::VIEWER, Value::Null).unwrap_err();
        assert!(matches!(e, Fail::Linear { not_found: true, .. }));
        let e = e.not_found("issue ENG-9", "find it");
        assert_eq!(e.to_string(), "not found: issue ENG-9 (or this API key cannot see it)");
        assert_eq!(e.hint_text(), Some("find it"));
        // The message offered to people, made safe for a terminal.
        let api = linear(|| {
            Response::json(
                400,
                r#"{"errors":[{"message":"x","extensions":{"userPresentableMessage":"Title is too long\u001b[31m"}}]}"#,
            )
        });
        let e = client(&r, &api.url).query(&mut r, crate::queries::VIEWER, Value::Null).unwrap_err();
        assert_eq!(e.to_string(), "Linear: Title is too long?[31m");
        // Errors beside data are the caller's.
        let api = linear(|| Response::json(200, r#"{"data":{"a":1},"errors":[{"message":"partly"}]}"#));
        let a = client(&r, &api.url).raw(&mut r, crate::queries::VIEWER, Value::Null).unwrap();
        assert_eq!((a.data["a"].as_i64(), a.errors.len()), (Some(1), 1));
        // An HTTP error without GraphQL errors.
        let api = linear(|| Response::json(500, "{}"));
        let e = client(&r, &api.url).query(&mut r, crate::queries::VIEWER, Value::Null).unwrap_err();
        assert_eq!(e.to_string(), "Linear answered HTTP 500");
        assert_eq!(e.hint_text(), Some(fail::RERUN));
        // A 4xx is the request's fault: a rerun would fail the same way.
        let api = linear(|| Response::json(404, "{}"));
        let e = client(&r, &api.url).query(&mut r, crate::queries::VIEWER, Value::Null).unwrap_err();
        assert_eq!((e.to_string().as_str(), e.hint_text()), ("Linear answered HTTP 404", None));
    }

    #[test]
    fn names_the_host_on_transport_failures() {
        let api = serve(|_| Response { status: 502, headers: vec![], body: "<html>bad gateway</html>".into() });
        let mut r = run();
        let e = client(&r, &api.url).query(&mut r, crate::queries::VIEWER, Value::Null).unwrap_err();
        assert!(e.to_string().contains("HTTP 502 without JSON"), "{e}");
        assert_eq!(e.hint_text(), Some(fail::RERUN));
        let e =
            client(&r, "http://127.0.0.1:9/graphql").query(&mut r, crate::queries::VIEWER, Value::Null).unwrap_err();
        assert!(e.to_string().starts_with("cannot reach 127.0.0.1:9: "), "{e}");
        assert_eq!(e.hint_text(), Some(fail::RERUN));
    }

    #[test]
    fn times_out() {
        let api = serve(|_| {
            std::thread::sleep(Duration::from_millis(1500));
            Response::json(200, "{}")
        });
        let mut r = run();
        r.timeout = 1;
        let e = client(&r, &api.url).query(&mut r, crate::queries::VIEWER, Value::Null).unwrap_err();
        assert_eq!(e.to_string(), format!("{} did not answer within 1 s; raise --timeout", host_of(&api.url)));
        assert_eq!(e.hint_text(), Some(fail::RERUN));
    }

    #[test]
    fn says_what_it_waits_on() {
        let api = serve(|_| {
            std::thread::sleep(Duration::from_millis(2300));
            Response::json(200, r#"{"data":{}}"#)
        });
        let mut r = run();
        let mut c = client(&r, &api.url);
        // Other tests may already have waited (on Windows, a refused
        // connection takes seconds): this one has its own once.
        static NOTED: AtomicBool = AtomicBool::new(false);
        c.noted = &NOTED;
        c.query(&mut r, crate::queries::VIEWER, Value::Null).unwrap();
        let err = std::mem::replace(&mut r.err, Sink::Buf(vec![])).into_buf();
        assert_eq!(String::from_utf8(err).unwrap(), format!("linearctl: waiting on {}\n", host_of(&api.url)));
    }

    #[test]
    fn reads_rate_limit_headers() {
        let api = linear(|| Response {
            status: 200,
            headers: vec![
                ("X-RateLimit-Requests-Remaining".into(), "2400".into()),
                ("X-RateLimit-Requests-Limit".into(), "2500".into()),
                ("X-RateLimit-Requests-Reset".into(), "1798000000000".into()),
                ("X-RateLimit-Complexity-Remaining".into(), "nope".into()),
                ("X-Complexity".into(), "12".into()),
            ],
            body: r#"{"data":{}}"#.into(),
        });
        let mut r = run();
        let c = client(&r, &api.url);
        assert!(!c.has_header());
        c.query(&mut r, crate::queries::VIEWER, Value::Null).unwrap();
        assert_eq!(
            c.meta(),
            json!({"requests": 1, "complexity": 12.0, "rateLimit": {"requests": {"remaining": 2400, "limit": 2500,
                "resetAt": "2026-12-23T04:26:40Z"}}})
        );
    }

    /// A command's requests share one connection: each new one would cost
    /// a TCP and TLS handshake, ~40 ms against api.linear.app.
    #[test]
    fn reuses_the_connection() {
        let api = linear(|| Response::json(200, r#"{"data":{}}"#));
        let mut r = run();
        let c = client(&r, &api.url);
        for _ in 0..3 {
            c.query(&mut r, crate::queries::VIEWER, Value::Null).unwrap();
        }
        assert_eq!(api.connections.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn downloads_with_the_key() {
        let files = serve(|req| match req.header("Authorization") {
            Some(KEY) => {
                Response { status: 200, headers: vec![("Content-Type".into(), "image/png".into())], body: "PNG".into() }
            }
            _ => Response { status: 401, headers: vec![], body: String::new() },
        });
        let base = files.url.trim_end_matches("/graphql");
        let mut r = run();
        let c = client(&r, &files.url);
        assert_eq!(c.download(&mut r, &format!("{base}/a/b")).unwrap(), (b"PNG".to_vec(), "image/png".to_string()));
        let mut wrong = client(&r, &files.url);
        wrong.key = "lin_api_wrong000000000000000".into();
        assert!(matches!(wrong.download(&mut r, &format!("{base}/a/b")), Err(Fail::Auth(_))));
        let missing = serve(|_| Response { status: 404, headers: vec![], body: String::new() });
        let base = missing.url.trim_end_matches("/graphql");
        let e = c.download(&mut r, &format!("{base}/x")).unwrap_err().to_string();
        assert!(e.starts_with("not found: "), "{e}");
        let broken = serve(|_| Response { status: 500, headers: vec![], body: String::new() });
        let base = broken.url.trim_end_matches("/graphql");
        let e = c.download(&mut r, &format!("{base}/x")).unwrap_err().to_string();
        assert!(e.ends_with("answered HTTP 500"), "{e}");
    }
}
