// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Eugene Butusov <ebutusov@gmail.com>

//! testport — a small connectivity-testing tool.
//!
//! The program is split into small modules, each with one job:
//! - `cli`: argument definitions (what the user can ask for)
//! - `tcp`: connection-oriented testing (handshake timing, banner, session)
//! - `udp`: connectionless testing (request/reply timing)
//!
//! This file holds `main` plus a few helpers shared by both protocols.

mod cli;
mod tcp;
mod udp;

use clap::Parser;
use cli::{Args, Protocol};
use std::error::Error;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::process::ExitCode;
use std::time::Duration;

/// A convenient alias: "either success, or any kind of error".
///
/// `Box<dyn Error>` lets `?` convert io errors, our own string messages, etc.
/// into one type. For a small binary this is perfectly fine; libraries usually
/// define their own error enums instead so callers can match on them.
pub type AppResult<T> = Result<T, Box<dyn Error>>;

fn main() -> ExitCode {
    // `parse()` handles -h/--help and invalid input itself (printing a message
    // and exiting), so after this line we know the arguments are valid.
    let args = Args::parse();

    // Keeping the real logic in `run` (which returns a Result) lets us use the
    // `?` operator everywhere and decide the exit code in a single place.
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Failed: {e}");
            // A non-zero exit code matters: connectivity checks are often used
            // in scripts, e.g. `testport db.local -p 5432 && ./deploy.sh`.
            ExitCode::FAILURE
        }
    }
}

fn run(args: &Args) -> AppResult<()> {
    println!("Trying {} port {}...", args.host, args.port);

    // Resolve the host name ONCE, before any timing starts. DNS lookups can
    // take tens of milliseconds; doing it inside the measurement loop would
    // make us measure the DNS server instead of the target.
    let addr = resolve(&args.host, args.port)?;
    if addr.ip().to_string() != args.host {
        println!("Resolved {} to {}", args.host, addr.ip());
    }

    match args.protocol() {
        Protocol::Tcp => tcp::run(addr, args),
        Protocol::Udp => udp::run(addr, args),
    }
}

/// Turns "host.com" + port into a concrete socket address.
///
/// `ToSocketAddrs` is implemented for `(&str, u16)`; it accepts both host names
/// and literal IPs and may return several addresses (IPv4 and IPv6, or
/// multiple A records).
fn resolve(host: &str, port: u16) -> AppResult<SocketAddr> {
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve '{host}': {e}"))?
        .collect();

    // Prefer IPv4 when available. On many systems "localhost" resolves to ::1
    // first, while lots of local services only listen on 127.0.0.1.
    addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or_else(|| addrs.first())
        .copied()
        .ok_or_else(|| format!("no addresses found for '{host}'").into())
}

/// Did this I/O error happen because a read timeout expired?
///
/// Portability trap: when a socket read timeout expires, Windows reports
/// `ErrorKind::TimedOut`, while Linux/macOS report `ErrorKind::WouldBlock`.
/// Checking only one of them makes the program break on the other platform.
pub fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
}

/// Formats a duration as milliseconds with one decimal, e.g. "10.3ms".
pub fn fmt_ms(d: Duration) -> String {
    format!("{:.1}ms", d.as_secs_f64() * 1000.0)
}

/// Prints the final line with average / min / max latency.
pub fn print_summary(samples: &[Duration], attempts: u32) {
    println!("{}", summary(samples, attempts));
}

/// Builds the summary line.
///
/// Keeping the *formatting* separate from the *printing* is a small habit that
/// pays off: a function returning a `String` is trivial to unit-test, while
/// testing what was written to stdout is awkward.
fn summary(samples: &[Duration], attempts: u32) -> String {
    // Pattern matching on `min()`/`max()` avoids `unwrap()`: an empty slice
    // gives `None`, and we simply have nothing to summarize.
    let (Some(min), Some(max)) = (samples.iter().min(), samples.iter().max()) else {
        return format!("Done, no successful probes out of {attempts}.");
    };
    let total: Duration = samples.iter().sum();
    let avg = total / samples.len() as u32;
    format!(
        "Done, avg latency is {} (min {}, max {}, {}/{} probes succeeded)",
        fmt_ms(avg),
        fmt_ms(*min),
        fmt_ms(*max),
        samples.len(),
        attempts
    )
}

// Unit tests live in the same file as the code they test, in a module that is
// compiled only for `cargo test` (`#[cfg(test)]`). Being a child module, it
// can access private items such as `resolve` and `summary` via `super::`.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_ms_uses_one_decimal() {
        assert_eq!(fmt_ms(Duration::from_micros(10_250)), "10.2ms");
        assert_eq!(fmt_ms(Duration::ZERO), "0.0ms");
        assert_eq!(fmt_ms(Duration::from_secs(2)), "2000.0ms");
    }

    #[test]
    fn summary_reports_avg_min_max() {
        let samples = [10, 20, 30].map(Duration::from_millis);
        assert_eq!(
            summary(&samples, 4),
            "Done, avg latency is 20.0ms (min 10.0ms, max 30.0ms, 3/4 probes succeeded)"
        );
    }

    #[test]
    fn summary_handles_no_samples() {
        assert_eq!(summary(&[], 4), "Done, no successful probes out of 4.");
    }

    #[test]
    fn timeout_detection_covers_both_platform_variants() {
        use io::ErrorKind::*;
        assert!(is_timeout(&io::Error::from(TimedOut))); // Windows
        assert!(is_timeout(&io::Error::from(WouldBlock))); // Linux/macOS
        assert!(!is_timeout(&io::Error::from(ConnectionRefused)));
    }

    #[test]
    fn resolve_accepts_literal_ips() {
        let addr = resolve("127.0.0.1", 8080).unwrap();
        assert_eq!(addr, "127.0.0.1:8080".parse().unwrap());

        let addr = resolve("::1", 53).unwrap();
        assert_eq!(addr, "[::1]:53".parse().unwrap());
    }

    #[test]
    fn resolve_prefers_ipv4_for_localhost() {
        let addr = resolve("localhost", 80).unwrap();
        assert!(addr.is_ipv4(), "expected IPv4, got {addr}");
    }

    #[test]
    fn resolve_reports_unknown_hosts() {
        // `.invalid` is a reserved TLD (RFC 2606) guaranteed never to resolve.
        let err = resolve("no-such-host.invalid", 80).unwrap_err();
        assert!(err.to_string().contains("cannot resolve"), "{err}");
    }
}
