//! A small HTTP/1.1 server for the client's tests: it answers each request
//! with a handler, keeps connections alive, and counts them.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub struct Request {
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Response {
    pub fn json(status: u16, body: &str) -> Response {
        Response { status, headers: vec![("Content-Type".into(), "application/json".into())], body: body.into() }
    }
}

pub struct Server {
    pub url: String,
    pub connections: Arc<AtomicUsize>,
    pub requests: Arc<Mutex<Vec<Request>>>,
}

pub fn serve(handler: impl Fn(&Request) -> Response + Send + Sync + 'static) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/graphql", listener.local_addr().unwrap());
    let connections = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(vec![]));
    let handler = Arc::new(handler);
    let (c, r) = (connections.clone(), requests.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            c.fetch_add(1, Ordering::SeqCst);
            let (handler, r) = (handler.clone(), r.clone());
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let mut headers = vec![];
                    let mut length = 0;
                    loop {
                        let mut h = String::new();
                        reader.read_line(&mut h).unwrap();
                        let h = h.trim_end();
                        if h.is_empty() {
                            break;
                        }
                        let (k, v) = h.split_once(':').unwrap();
                        if k.eq_ignore_ascii_case("content-length") {
                            length = v.trim().parse().unwrap();
                        }
                        headers.push((k.to_string(), v.trim().to_string()));
                    }
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    let req = Request { headers, body: String::from_utf8_lossy(&body).into_owned() };
                    let res = handler(&req);
                    r.lock().unwrap().push(req);
                    let mut out = format!("HTTP/1.1 {} X\r\nContent-Length: {}\r\n", res.status, res.body.len());
                    for (k, v) in &res.headers {
                        out.push_str(&format!("{k}: {v}\r\n"));
                    }
                    out.push_str("\r\n");
                    out.push_str(&res.body);
                    if stream.write_all(out.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    Server { url, connections, requests }
}
