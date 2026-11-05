//! Command-line interface definition.
//!
//! We use `clap` with its *derive* API: you describe the arguments as a plain
//! struct, and clap generates the parser, validation, `-h/--help` and
//! `--version` for you. This is the idiomatic way to build CLIs in Rust —
//! hand-rolled `std::env::args()` parsing quickly becomes a bug farm.

use clap::{ArgGroup, Parser, value_parser};
use std::time::Duration;

/// Text printed by `-V/--version`: the version plus the author.
///
/// Cargo passes metadata from `Cargo.toml` to the compiler as environment
/// variables (`CARGO_PKG_VERSION`, `CARGO_PKG_AUTHORS`...). `env!` reads them
/// at *compile time* and `concat!` joins them into one `&'static str`, so the
/// values live in `Cargo.toml` only and can never drift out of sync.
const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"), " ",
    env!("CARGO_PKG_AUTHORS")
);

/// Test TCP/UDP connectivity to a port: connect time, latency, banner,
/// and an interactive telnet-like mode.
#[derive(Parser, Debug)]
#[command(name = "testport", version = VERSION, about)]
// An ArgGroup with the default `multiple(false)` makes -t and -u mutually
// exclusive: clap itself rejects `-t -u` with a helpful error message.
#[command(group(ArgGroup::new("protocol").args(["tcp", "udp"])))]
pub struct Args {
    /// Host name or IP address to test
    pub host: String,

    /// Port number to connect to (1-65535)
    // `u16` already limits the value to 0..=65535; `.range(1..)` also rejects
    // port 0, which is not a valid destination port.
    #[arg(short = 'p', long, value_parser = value_parser!(u16).range(1..))]
    pub port: u16,

    /// Use TCP (the default when neither -t nor -u is given)
    // These are plain `bool` flags, NOT options taking a value. If `-u` took a
    // value, `-u host.com -p 12` would swallow `host.com` as that value.
    #[arg(short = 't', long)]
    pub tcp: bool,

    /// Use UDP
    #[arg(short = 'u', long)]
    pub udp: bool,

    /// Print the banner (first data) sent by the remote service
    #[arg(short = 'b', long)]
    pub banner: bool,

    /// Stay connected after the test and forward stdin to the remote host
    /// (a raw session: no telnet option negotiation is performed)
    #[arg(long)]
    pub telnet: bool,

    /// Number of latency probes to send
    #[arg(short = 'c', long, default_value_t = 4, value_parser = value_parser!(u32).range(1..))]
    pub count: u32,

    /// Timeout for connecting / waiting for a reply, in milliseconds
    // A zero timeout must be rejected: `TcpStream::connect_timeout` and
    // `set_read_timeout` both return an error for `Duration::ZERO`.
    // Why 3 s and not less: on Windows, when a connection attempt is refused
    // (RST), the OS silently retries for ~2 s before reporting "refused". A
    // shorter timeout would make closed ports look like filtered ones.
    #[arg(short = 'w', long = "timeout", default_value_t = 3000,
          value_parser = value_parser!(u64).range(1..))]
    pub timeout_ms: u64,
}

/// The transport protocol, derived from the -t / -u flags.
///
/// Turning two booleans into an enum early means the rest of the program can
/// `match` on it, and the compiler guarantees every protocol is handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Tcp,
    Udp,
}

impl Args {
    pub fn protocol(&self) -> Protocol {
        if self.udp {
            Protocol::Udp
        } else {
            Protocol::Tcp
        }
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `try_parse_from` is the testable twin of `parse()`: it takes the
    /// arguments explicitly (first item = program name) and returns a `Result`
    /// instead of printing an error and exiting the process.
    fn parse(args: &[&str]) -> Result<Args, clap::Error> {
        Args::try_parse_from(std::iter::once("testport").chain(args.iter().copied()))
    }

    #[test]
    fn clap_definition_is_valid() {
        // Catches mistakes in the derive attributes (duplicate short flags,
        // bad group members...) that clap would otherwise report only at runtime.
        use clap::CommandFactory;
        Args::command().debug_assert();
    }

    #[test]
    fn parses_spec_example() {
        let args = parse(&["-u", "host.com", "-p", "12"]).unwrap();
        assert_eq!(args.host, "host.com");
        assert_eq!(args.port, 12);
        assert_eq!(args.protocol(), Protocol::Udp);
    }

    #[test]
    fn defaults_to_tcp_with_sensible_defaults() {
        let args = parse(&["host.com", "-p", "80"]).unwrap();
        assert_eq!(args.protocol(), Protocol::Tcp);
        assert_eq!(args.count, 4);
        assert_eq!(args.timeout(), Duration::from_millis(3000));
        assert!(!args.banner && !args.telnet);
    }

    #[test]
    fn parses_all_flags() {
        let args = parse(&[
            "-t", "-b", "--telnet", "-c", "10", "-w", "500", "h", "-p", "22",
        ])
        .unwrap();
        assert_eq!(args.protocol(), Protocol::Tcp);
        assert!(args.banner && args.telnet);
        assert_eq!(args.count, 10);
        assert_eq!(args.timeout(), Duration::from_millis(500));
    }

    #[test]
    fn tcp_and_udp_are_mutually_exclusive() {
        let err = parse(&["-t", "-u", "h", "-p", "1"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn rejects_invalid_values() {
        // Each case must fail; `{case:?}` in the message shows which one didn't.
        for case in [
            &["h", "-p", "0"][..], // port 0
            &["h", "-p", "65536"], // port out of u16 range
            &["h", "-p", "http"],  // not a number
            &["h", "-p", "80", "-c", "0"],
            &["h", "-p", "80", "-w", "0"],
            &["h"],        // missing port
            &["-p", "80"], // missing host
        ] {
            assert!(parse(case).is_err(), "expected error for {case:?}");
        }
    }

    #[test]
    fn help_is_available_via_dash_h() {
        let err = parse(&["-h"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
    }
}
