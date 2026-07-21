//! A tiny, dependency-free HTTP/1.1 layer for the living-service demo: a
//! thread-per-connection server that scores each request through Blaze, and a
//! keep-alive load generator that keeps thousands of requests per second
//! flowing at it. Nothing here is a general HTTP implementation — it speaks
//! exactly the one request the demo sends (`GET /score?amount=&velocity=&age=`)
//! and frames responses by `Content-Length` so a persistent connection stays in
//! sync.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::service::{Inputs, Service};

/// Stand up the scoring service on an ephemeral loopback port. Returns the bound
/// address; a detached acceptor spawns one handler thread per connection, each
/// resolving its own `FuncHandle` and serving requests until the peer hangs up.
pub fn serve(service: Arc<Service>) -> std::io::Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let service = service.clone();
            thread::spawn(move || {
                let _ = handle_connection(&service, stream);
            });
        }
    });
    Ok(addr)
}

/// Serve every request on one keep-alive connection until EOF or an IO error.
fn handle_connection(service: &Service, stream: TcpStream) -> std::io::Result<()> {
    stream.set_nodelay(true).ok();
    // One handle per connection thread — the lock-free fast path a real
    // deployment resolves once and reuses for the life of the worker.
    let mut handle = match service.score_handle() {
        Ok(h) => h,
        Err(_) => return Ok(()),
    };
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;

    loop {
        // Request line, e.g. `GET /score?amount=1&velocity=2&age=3 HTTP/1.1`.
        let mut request_line = String::new();
        if reader.read_line(&mut request_line)? == 0 {
            return Ok(()); // peer closed
        }
        // Drain headers up to the blank line (we need none of them).
        loop {
            let mut header = String::new();
            let n = reader.read_line(&mut header)?;
            if n == 0 || header == "\r\n" || header == "\n" {
                break;
            }
        }

        let target = request_line.split_whitespace().nth(1).unwrap_or("/");
        let inputs = parse_query(target);
        let result = service.handle_score(&mut handle, inputs);

        let (status, body) = match result {
            Ok(score) => ("200 OK", format!("{{\"score\":{score}}}")),
            // The stable entry-point ABI makes this unreachable across the demo's
            // swaps — but if a request ever failed, the service says so honestly
            // rather than pretending, and the failed call is already counted.
            Err(e) => ("503 Service Unavailable", format!("{{\"error\":\"{e}\"}}")),
        };
        write!(
            writer,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{}",
            body.len(),
            body
        )?;
        writer.flush()?;
    }
}

/// Parse `/score?amount=1&velocity=2&age=3` into [`Inputs`]; any missing or
/// malformed field defaults to 0.
fn parse_query(target: &str) -> Inputs {
    let mut inputs = Inputs { amount: 0, velocity: 0, age: 0 };
    if let Some((_, query)) = target.split_once('?') {
        for pair in query.split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                let v = value.parse::<i64>().unwrap_or(0);
                match key {
                    "amount" => inputs.amount = v,
                    "velocity" => inputs.velocity = v,
                    "age" => inputs.age = v,
                    _ => {}
                }
            }
        }
    }
    inputs
}

/// A deterministic LCG so the generated load is reproducible run to run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % (hi - lo) as u64) as i64
    }
    /// A request spread wide enough to straddle every threshold the story moves:
    /// the two amount tiers, the age window (30 and 90 days), and the velocity
    /// cap — so divergences and behavior changes are actually exercised.
    fn request(&mut self) -> Inputs {
        Inputs {
            amount: self.range(0, 200_000),
            velocity: self.range(0, 12),
            age: self.range(0, 120),
        }
    }
}

/// Spawn `n` keep-alive HTTP clients hammering `addr` until `stop` is set. Each
/// holds one persistent connection and reconnects if it drops, so the load stays
/// steady even across the roughest beat.
pub fn spawn_load(addr: SocketAddr, n: usize, stop: Arc<AtomicBool>) -> Vec<JoinHandle<()>> {
    (0..n)
        .map(|i| {
            let stop = stop.clone();
            thread::spawn(move || {
                let mut rng = Rng(0xC0FFEE ^ (i as u64).wrapping_mul(0x9E3779B97F4A7C15));
                while !stop.load(Ordering::Relaxed) {
                    if let Err(()) = drive_connection(addr, &stop, &mut rng) {
                        // The connection dropped; pause briefly and reconnect.
                        thread::sleep(Duration::from_millis(1));
                    }
                }
            })
        })
        .collect()
}

/// Open one connection and pump requests down it until `stop`, or an IO error
/// bubbles up (signalling the caller to reconnect).
fn drive_connection(addr: SocketAddr, stop: &AtomicBool, rng: &mut Rng) -> Result<(), ()> {
    let stream = TcpStream::connect(addr).map_err(|_| ())?;
    stream.set_nodelay(true).ok();
    let mut reader = BufReader::new(stream.try_clone().map_err(|_| ())?);
    let mut writer = stream;

    while !stop.load(Ordering::Relaxed) {
        let inp = rng.request();
        write!(
            writer,
            "GET /score?amount={}&velocity={}&age={} HTTP/1.1\r\nHost: localhost\r\n\r\n",
            inp.amount, inp.velocity, inp.age
        )
        .map_err(|_| ())?;
        writer.flush().map_err(|_| ())?;
        read_response(&mut reader)?;
    }
    Ok(())
}

/// Read one HTTP response fully (status line, headers, then exactly
/// `Content-Length` body bytes), so the persistent connection is left framed for
/// the next request.
fn read_response(reader: &mut BufReader<TcpStream>) -> Result<(), ()> {
    let mut status = String::new();
    if reader.read_line(&mut status).map_err(|_| ())? == 0 {
        return Err(()); // peer closed
    }
    let mut content_length = 0usize;
    loop {
        let mut header = String::new();
        let n = reader.read_line(&mut header).map_err(|_| ())?;
        if n == 0 {
            return Err(());
        }
        if header == "\r\n" || header == "\n" {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).map_err(|_| ())?;
    Ok(())
}
