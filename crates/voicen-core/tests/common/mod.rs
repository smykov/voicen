//! Shared helpers of the voicen-core test binaries: for `tests/local_download.rs`,
//! `tests/local_download_refused.rs` and `tests/local_store.rs` (T-016) the fake
//! model, test catalog entries, a fake disk probe and a raw-TCP mock model server
//! (the download harness is in [`download`]); for `tests/openai_client.rs` and
//! `tests/api_pipeline.rs` the refused address [`refused_addr`] (T-047).
//!
//! The fake model is 65 600 bytes (~64 KiB; size + 1 % is a whole number). Its
//! SHA-256 was computed once on the host with `sha256sum` over the same byte
//! formula and is hard-coded: the tests do not hash (no `sha2` dependency; the
//! owner question of decision #49). Fake data only: 127.0.0.1, `SECRETQ`.
#![allow(dead_code)]

pub mod download;
#[cfg(test)]
mod refused_addr_tests;

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use voicen_core::local_models::catalog::{CatalogEntry, ModelId};
use voicen_core::local_models::download::DiskSpace;

/// Catalog size of the fake model.
pub const SIZE: u64 = 65_600;
/// SHA-256 of [`model_bytes`] (`sha256sum`, computed once on the host).
pub const SHA256: &str = "5fa1d43f72ad10b5335bebd2a436ae835d64256e27a25c448a284ba46665f135";
/// Catalog size + 1 % (R-9).
pub const NEEDED: u64 = 66_256;
/// A query value that must never reach an event, a reason or its Debug text.
pub const QUERY_SECRET: &str = "SECRETQ";
/// A path segment of every test URL; must not reach a reason either.
pub const URL_PATH_MARK: &str = "resolve-mark";

/// The fake model's bytes: `(i * 31 + (i >> 8) * 7) & 0xFF`.
pub fn model_bytes() -> Vec<u8> {
    (0..SIZE as usize)
        .map(|i| ((i * 31 + (i >> 8) * 7) & 0xFF) as u8)
        .collect()
}

pub fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// A catalog entry for the fake model under `id`, served from `url`.
pub fn entry(id: ModelId, file_name: &'static str, url: &str) -> CatalogEntry {
    CatalogEntry {
        id,
        file_name,
        url: leak(url.to_string()),
        size_bytes: SIZE,
        sha256: SHA256,
        recommended: false,
    }
}

pub fn catalog(entries: Vec<CatalogEntry>) -> &'static [CatalogEntry] {
    Box::leak(entries.into_boxed_slice())
}

/// File names in `dir`, sorted (empty when the dir is missing).
pub fn dir_entries(dir: &Path) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// ---- disk probe -------------------------------------------------------------

/// A disk probe with a fixed answer that records the directories it was asked about.
pub struct FakeDisk {
    answer: Mutex<Result<u64, io::ErrorKind>>,
    asked: Mutex<Vec<PathBuf>>,
}

impl FakeDisk {
    pub fn with_available(bytes: u64) -> Arc<FakeDisk> {
        Arc::new(FakeDisk {
            answer: Mutex::new(Ok(bytes)),
            asked: Mutex::new(Vec::new()),
        })
    }

    pub fn failing() -> Arc<FakeDisk> {
        Arc::new(FakeDisk {
            answer: Mutex::new(Err(io::ErrorKind::Other)),
            asked: Mutex::new(Vec::new()),
        })
    }

    pub fn asked(&self) -> Vec<PathBuf> {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl DiskSpace for FakeDisk {
    fn available_bytes(&self, dir: &Path) -> io::Result<u64> {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(dir.to_path_buf());
        (*self.answer.lock().unwrap_or_else(PoisonError::into_inner)).map_err(io::Error::from)
    }
}

// ---- raw model server -------------------------------------------------------

/// How one connection is answered.
#[derive(Debug, Clone)]
pub enum Serve {
    /// 200, `Content-Length: SIZE`, the whole model.
    Full,
    /// This status with a short body.
    Status(u16),
    /// The whole model with one byte flipped (same length).
    Altered,
    /// `Content-Length: SIZE - 100` and exactly those bytes (a clean, short end).
    Short,
    /// The model plus one extra byte (`Content-Length: SIZE + 1`).
    Long,
    /// `Content-Length: SIZE`, half the bytes, then close (FIN).
    CutOff,
    /// Chunked transfer, a few chunks, then close without the terminating chunk.
    DropChunked,
    /// `Content-Length: SIZE`, `after` bytes, then nothing for `hold`, then close.
    Stall { after: usize, hold: Duration },
    /// Reads the request and sends nothing (not even headers) for `hold`.
    SilentHeaders { hold: Duration },
    /// `Content-Length: SIZE`, `chunk` bytes every `every` until done.
    Trickle { chunk: usize, every: Duration },
}

/// A loopback server answering connection `n` with `plan[n]`, and [`Serve::Full`]
/// after the plan. Every connection runs on its own thread, so a held connection
/// never delays the next one (a retry).
pub struct Server {
    pub port: u16,
    accepts: Arc<AtomicUsize>,
}

impl Server {
    pub fn start(plan: Vec<Serve>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
        Server::on(listener, plan)
    }

    /// Serves on `port` (after a refused-port phase). Panics if the port was taken
    /// in between (a test-environment race, not a product failure). Used only by
    /// `tests/local_download_refused.rs`, whose process binds no other port.
    pub fn start_on(port: u16, plan: Vec<Serve>) -> Server {
        let listener = TcpListener::bind(("127.0.0.1", port)).expect("re-bind the refused port");
        Server::on(listener, plan)
    }

    fn on(listener: TcpListener, plan: Vec<Serve>) -> Server {
        let port = listener.local_addr().expect("local addr").port();
        let accepts = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&accepts);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let n = counter.fetch_add(1, Ordering::SeqCst);
                let serve = plan.get(n).cloned().unwrap_or(Serve::Full);
                std::thread::spawn(move || answer(stream, serve));
            }
        });
        Server { port, accepts }
    }

    /// The model URL, with a path mark and a secret query that must never surface.
    pub fn url(&self, file: &str) -> String {
        url_for(self.port, file)
    }

    /// Connections accepted so far.
    pub fn accepts(&self) -> usize {
        self.accepts.load(Ordering::SeqCst)
    }
}

/// The one loopback address a test may expect to be refused (T-047, decision #53):
/// `127.0.0.1:1`.
///
/// Invariant: never a port that was bound and released. Port 1 lies below the OS
/// ephemeral range (Linux `ip_local_port_range`, 32768–60999 by default; Windows
/// dynamic range, 49152–65535 by default), so no `bind(0)` or `connect()` of a
/// sibling test in the same process can be handed it. That nothing listens there
/// is checked at each call: a plain connect must be refused, otherwise this panics
/// (a test-environment problem, reported loudly instead of a flaky result).
/// Tests: `refused_addr_tests.rs`.
pub fn refused_addr() -> SocketAddr {
    let addr = SocketAddr::from(([127, 0, 0, 1], 1));
    match TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
        Ok(_) => panic!(
            "refused_addr: something listens on {addr}; the refused-port tests need \
             nothing listening there (T-047, decision #53)"
        ),
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => addr,
        Err(e) => panic!(
            "refused_addr: a connect probe to {addr} was not refused ({:?}: {e}); \
             the refused-port tests need an immediate refusal (T-047, decision #53)",
            e.kind()
        ),
    }
}

pub fn url_for(port: u16, file: &str) -> String {
    format!("http://127.0.0.1:{port}/{URL_PATH_MARK}/{file}?download=true&sig={QUERY_SECRET}")
}

/// Reads the request head (a GET has no body).
fn read_head(s: &mut TcpStream) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

fn head(status: &str, length: usize) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
    )
}

fn answer(mut s: TcpStream, serve: Serve) {
    read_head(&mut s);
    let body = model_bytes();
    let size = body.len();
    let _ = match serve {
        Serve::Full => s
            .write_all(head("200 OK", size).as_bytes())
            .and_then(|()| s.write_all(&body)),
        Serve::Status(code) => {
            let text = b"no model here";
            s.write_all(head(&format!("{code} Test"), text.len()).as_bytes())
                .and_then(|()| s.write_all(text))
        }
        Serve::Altered => {
            let mut b = body;
            b[size / 3] ^= 0x01;
            s.write_all(head("200 OK", size).as_bytes())
                .and_then(|()| s.write_all(&b))
        }
        Serve::Short => s
            .write_all(head("200 OK", size - 100).as_bytes())
            .and_then(|()| s.write_all(&body[..size - 100])),
        Serve::Long => {
            let mut b = body;
            b.push(0x42);
            s.write_all(head("200 OK", size + 1).as_bytes())
                .and_then(|()| s.write_all(&b))
        }
        Serve::CutOff => s
            .write_all(head("200 OK", size).as_bytes())
            .and_then(|()| s.write_all(&body[..size / 2])),
        Serve::DropChunked => {
            let h = "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
            let mut r = s.write_all(h.as_bytes());
            for part in body[..size / 2].chunks(4096) {
                r = r
                    .and_then(|()| s.write_all(format!("{:x}\r\n", part.len()).as_bytes()))
                    .and_then(|()| s.write_all(part))
                    .and_then(|()| s.write_all(b"\r\n"));
            }
            r
        }
        Serve::Stall { after, hold } => {
            let r = s
                .write_all(head("200 OK", size).as_bytes())
                .and_then(|()| s.write_all(&body[..after]))
                .and_then(|()| s.flush());
            std::thread::sleep(hold);
            r
        }
        Serve::SilentHeaders { hold } => {
            std::thread::sleep(hold);
            Ok(())
        }
        Serve::Trickle { chunk, every } => {
            let mut r = s.write_all(head("200 OK", size).as_bytes());
            for part in body.chunks(chunk) {
                r = r.and_then(|()| s.write_all(part)).and_then(|()| s.flush());
                if r.is_err() {
                    break;
                }
                std::thread::sleep(every);
            }
            r
        }
    };
    let _ = s.flush();
    let _ = s.shutdown(Shutdown::Both);
}
