# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`testport` is a Rust CLI for connectivity testing: it connects to a host's port over TCP or UDP and reports reachability, connect time and latency. It can also print the service banner (`-b`) and has an interactive raw session mode (`--telnet`). The only dependency is `clap` (derive API). User-facing documentation is in `README.md`.

## Commands

```sh
cargo build --release                     # binary: target/release/testport(.exe)
cargo test                                # unit + integration tests (~3 s)
cargo test resolve_prefers                # tests whose name contains a substring
cargo test --test cli tcp_banner          # one integration test
cargo test --bin testport cli::tests      # unit tests of one module
cargo clippy --all-targets                # must stay warning-free
cargo fmt
```

## Code style requirements

- Comments are meant to teach: explain Rust idioms and network-programming points (why something is done, platform traps), not restate the code.
- Prefer clean, simple code over abstraction. Use `Box<dyn Error>` (`AppResult` in `main.rs`) and `?`, not `anyhow`/`thiserror`.
- Keep unit tests in a `#[cfg(test)] mod tests` at the **end** of each file (clippy flags items after the test module).

## Architecture

- `main.rs` parses arguments, resolves the host **once, before any timing** (preferring IPv4, because `localhost` often resolves to `::1` first), then dispatches on `cli::Protocol` to `tcp::run` or `udp::run`. Errors bubble up to `main`, which prints `Failed: …` and exits 1. clap exits 2 on bad arguments. Scripts rely on these exit codes.
- `main.rs` also holds the helpers shared by both protocols: `is_timeout`, `fmt_ms`, `print_summary`. Formatting (`summary`) is kept separate from printing so it can be unit-tested.
- **TCP** (`tcp.rs`): one initial connect, then `count` probes, each a fresh `connect_timeout` that is dropped immediately (the handshake time is the latency). The banner and telnet use a separate session connection opened afterwards.
- **UDP** (`udp.rs`): `connect()` only sets the peer. Each probe sends `PROBE_PAYLOAD` and waits for a reply. The outcomes are `Reply::{Data, NoResponse, PortUnreachable}`. Silence means "open|filtered" and exits 0. Only an ICMP unreachable is a failure.
- **Telnet mode** (both protocols): `try_clone()` the socket. A reader thread copies the socket to stdout, and the main thread sends stdin lines (`\r\n` for TCP, `\n` for UDP). In TCP mode, the reader calls `process::exit` when the server closes the connection, because a blocking stdin read can't be interrupted. On stdin EOF the program sleeps for `timeout` instead of calling `shutdown(Write)`: example.com drops half-closed connections without replying.

## Platform behavior that must be preserved

- Read timeouts are reported as `TimedOut` on Windows and `WouldBlock` on Unix, so always use `is_timeout`.
- ICMP port unreachable on UDP is reported as `ConnectionReset` on Windows and `ConnectionRefused` on Unix (`is_port_unreachable`). It can arrive on either `send` or `recv`.
- Windows retries a refused TCP connect for about 2 s. That is why the default timeout is 3000 ms. The `tcp_closed_port_fails_with_refused` integration test depends on it.
- Clear the read timeout (`set_read_timeout(None)`) before starting a telnet reader thread. The banner read sets one.

## Tests

`tests/cli.rs` runs the real binary (`env!("CARGO_BIN_EXE_testport")`) against in-process TCP/UDP servers bound to port 0, so tests can run in parallel. Closed ports are obtained by binding to a port and then dropping the socket. Telnet tests pipe stdin through `testport_with_input`. Tests must not depend on external hosts. The exception is `no-such-host.invalid`, which relies on the reserved `.invalid` TLD never resolving.

The original spec (`specs.md`) is gitignored and may not be present.
