//! TCP testing.
//!
//! TCP is connection-oriented: before any data flows, client and server do a
//! three-way handshake (SYN -> SYN/ACK -> ACK). `connect()` returns once that
//! handshake completes, so the time `connect()` takes is roughly one network
//! round trip — a good latency measurement that works on ANY open TCP port,
//! without the service having to understand a single byte we send.

use crate::cli::Args;
use crate::{AppResult, fmt_ms, is_timeout, print_summary};
use std::io::{self, BufRead, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};
use std::{process, thread};

/// Pause between probes, so we don't hammer the server with connections.
const PROBE_INTERVAL: Duration = Duration::from_millis(250);

pub fn run(addr: SocketAddr, args: &Args) -> AppResult<()> {
    let timeout = args.timeout();

    // First connection: is the port reachable at all?
    // `Instant` is a monotonic clock — always use it (not `SystemTime`) to
    // measure elapsed time, since wall-clock time can jump (NTP, DST...).
    let start = Instant::now();
    let first = connect(addr, timeout)?;
    println!(
        "Connected in {}, measuring times...",
        fmt_ms(start.elapsed())
    );
    // Dropping a `TcpStream` closes the connection. Rust has no explicit
    // `close()` — resources are released when their owner goes out of scope
    // (RAII). `drop()` just makes that happen right now.
    drop(first);

    // Latency probes: each one is a fresh handshake, closed immediately.
    let mut samples = Vec::new();
    for i in 1..=args.count {
        thread::sleep(PROBE_INTERVAL);
        let start = Instant::now();
        match connect(addr, timeout) {
            Ok(_stream) => {
                // `_stream` is dropped at the end of this arm -> connection closed.
                let rtt = start.elapsed();
                println!("-> {i} {}", fmt_ms(rtt));
                samples.push(rtt);
            }
            Err(e) => println!("-> {i} {e}"),
        }
    }
    print_summary(&samples, args.count);
    if samples.is_empty() {
        return Err("all latency probes failed".into());
    }

    // Banner and telnet mode use one separate, longer-lived "session" connection.
    if args.banner || args.telnet {
        let mut session = connect(addr, timeout)?;
        if args.banner {
            print_banner(&mut session, timeout)?;
        }
        if args.telnet {
            telnet(session, timeout)?;
        }
    }
    Ok(())
}

/// Connects with a timeout and turns common failures into readable messages.
///
/// Plain `TcpStream::connect` uses the OS default timeout, which can be 20+
/// seconds. For a testing tool, a user-controlled timeout is much nicer.
fn connect(addr: SocketAddr, timeout: Duration) -> AppResult<TcpStream> {
    TcpStream::connect_timeout(&addr, timeout).map_err(|e| {
        let msg = match e.kind() {
            // The host answered with RST: it is up, but nothing listens there.
            io::ErrorKind::ConnectionRefused => "connection refused (port closed)".to_string(),
            // No answer at all: host down, or a firewall silently drops packets.
            // (On Windows a refused port can also end up here if the timeout
            // is under ~2 s, because Windows retries refused connections.)
            _ if is_timeout(&e) => format!(
                "timed out after {} (host down or port filtered)",
                fmt_ms(timeout)
            ),
            _ => e.to_string(),
        };
        msg.into()
    })
}

/// Reads whatever the server sends first (SSH, SMTP, FTP... greet the client).
fn print_banner(stream: &mut TcpStream, timeout: Duration) -> io::Result<()> {
    // Without a read timeout, `read()` would block forever on services that
    // wait for the client to speak first (HTTP is the classic example).
    stream.set_read_timeout(Some(timeout))?;

    let mut buf = [0u8; 4096];
    match stream.read(&mut buf) {
        // `Ok(0)` is how Rust (and the OS) signals end-of-stream: peer closed.
        Ok(0) => println!("Banner: (connection closed by server without data)"),
        Ok(n) => {
            // Network data is just bytes and may not be valid UTF-8.
            // `from_utf8_lossy` replaces invalid sequences instead of failing.
            let text = String::from_utf8_lossy(&buf[..n]);
            println!("Banner:\n{}", text.trim_end());
        }
        Err(e) if is_timeout(&e) => println!(
            "Banner: (none received within {}; the service may wait for the client to speak first)",
            fmt_ms(timeout)
        ),
        Err(e) => return Err(e),
    }
    Ok(())
}

/// Interactive session: stdin lines go to the server, server data goes to stdout.
///
/// This is a "raw" session like `telnet host port` is typically used for
/// testing: we do not implement the Telnet protocol's option negotiation (IAC).
fn telnet(stream: TcpStream, timeout: Duration) -> AppResult<()> {
    // Important: the banner read set a timeout. Clear it, otherwise the reader
    // thread would give up the first time the server is quiet for a while.
    stream.set_read_timeout(None)?;

    // We need to read and write at the same time, and both directions block.
    // The simplest solution is two threads. `try_clone()` creates a second
    // handle to the SAME OS socket, so one thread can own the reading half
    // and another the writing half — Rust's ownership rules are satisfied
    // because each thread owns its own `TcpStream` value.
    let mut reader = stream.try_clone()?;
    let mut writer = stream;

    println!(
        "Telnet mode: type lines to send; EOF (Ctrl+D, or Ctrl+Z then Enter on Windows) to quit."
    );

    // `move` transfers ownership of `reader` into the new thread.
    // We never join this thread: it ends the whole process when the server
    // closes the connection (see below).
    thread::spawn(move || {
        let mut stdout = io::stdout();
        let mut buf = [0u8; 4096];
        // `loop` is an expression: `break value` makes it evaluate to `value`,
        // here the exit code (0 = clean close, 1 = error).
        let code = loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    println!("\nConnection closed by remote host.");
                    break 0;
                }
                Err(e) => {
                    println!("\nConnection lost: {e}");
                    break 1;
                }
                Ok(n) => {
                    // Write raw bytes (no UTF-8 conversion) and flush right
                    // away: stdout is line-buffered, but server output often
                    // doesn't end with a newline (e.g. a "login:" prompt).
                    let _ = stdout.write_all(&buf[..n]);
                    let _ = stdout.flush();
                }
            }
        };
        // The main thread may be blocked waiting for keyboard input, and there
        // is no portable way to interrupt a blocking stdin read. Exiting the
        // process is the pragmatic way to end the session.
        process::exit(code);
    });

    // `lines()` strips the trailing "\n" or "\r\n"; we re-add "\r\n", the line
    // ending most text protocols (Telnet, HTTP, SMTP, FTP...) expect.
    for line in io::stdin().lock().lines() {
        let line = line?;
        writer.write_all(line.as_bytes())?;
        writer.write_all(b"\r\n")?;
    }

    // stdin hit EOF (e.g. input was piped in). Keep the connection open for up
    // to `timeout` so the server's response can arrive; if the server closes
    // first, the reader thread ends the process earlier.
    //
    // Why not `shutdown(Shutdown::Write)` (a TCP "half-close")? In theory it
    // tells the server "I'm done sending, go ahead and answer". In practice
    // many servers and proxies treat it as the client going away and drop the
    // connection without answering, so waiting is more robust.
    thread::sleep(timeout);
    println!("\nNo more input, closing connection.");
    Ok(())
}
