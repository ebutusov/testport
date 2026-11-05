//! End-to-end tests: run the real `testport` binary against local servers.
//!
//! Files in `tests/` are *integration tests*: each one is compiled as a
//! separate crate that can only see the program from the outside, exactly like
//! a user would. Cargo builds the binary first and tells us where it is through
//! the `CARGO_BIN_EXE_<name>` environment variable.
//!
//! Every server binds to port 0 ("any free port"), so tests never collide with
//! real services or with each other when cargo runs them in parallel.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, UdpSocket};
use std::process::{Command, Output, Stdio};
use std::thread;

/// Runs testport with the given arguments and waits for it to finish.
fn testport(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_testport"))
        .args(args)
        .stdin(Stdio::null()) // never wait for keyboard input in tests
        .output()
        .expect("failed to run testport")
}

/// Runs testport, feeding `input` to its stdin.
fn testport_with_input(args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_testport"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start testport");
    // `take()` moves stdin out of the child; dropping it at the end of the
    // statement closes the pipe, which the program sees as EOF.
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().expect("testport did not finish")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A tiny line-based TCP server: sends a greeting, answers each line with
/// "ECHO <line>", and says "bye" and closes on "quit". Returns the port.
fn tcp_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            // One thread per connection, so latency probes (which connect and
            // immediately hang up) never block the real session.
            thread::spawn(move || {
                let mut writer = stream.try_clone().unwrap();
                let _ = writer.write_all(b"220 test server\r\n");
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { break };
                    if line == "quit" {
                        let _ = writer.write_all(b"bye\r\n");
                        break;
                    }
                    let _ = writer.write_all(format!("ECHO {line}\r\n").as_bytes());
                }
                // Dropping `writer` closes the connection.
            });
        }
    });
    port
}

/// A UDP server that answers every datagram with "ECHO <datagram>".
fn udp_echo_server() -> u16 {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    thread::spawn(move || {
        let mut buf = [0u8; 2048];
        while let Ok((n, peer)) = socket.recv_from(&mut buf) {
            let reply = [b"ECHO ", &buf[..n]].concat();
            let _ = socket.send_to(&reply, peer);
        }
    });
    port
}

/// A port that nothing listens on: bind to a free port, then release it.
fn closed_tcp_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn closed_udp_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

// ---------------------------------------------------------------- TCP

#[test]
fn tcp_open_port_reports_latency() {
    let port = tcp_server().to_string();
    let out = testport(&["127.0.0.1", "-p", &port, "-c", "3", "-w", "1000"]);
    let text = stdout(&out);

    assert!(
        out.status.success(),
        "stdout:\n{text}\nstderr:\n{}",
        stderr(&out)
    );
    assert!(
        text.contains(&format!("Trying 127.0.0.1 port {port}...")),
        "{text}"
    );
    assert!(text.contains("Connected in"), "{text}");
    for i in 1..=3 {
        assert!(
            text.contains(&format!("-> {i} ")),
            "missing probe {i}:\n{text}"
        );
    }
    assert!(text.contains("3/3 probes succeeded"), "{text}");
}

#[test]
fn tcp_banner_is_printed_with_dash_b() {
    let port = tcp_server().to_string();
    let out = testport(&["127.0.0.1", "-p", &port, "-b", "-c", "1", "-w", "1000"]);
    let text = stdout(&out);

    assert!(out.status.success(), "{text}");
    assert!(text.contains("Banner:\n220 test server"), "{text}");
}

#[test]
fn tcp_closed_port_fails_with_refused() {
    let port = closed_tcp_port().to_string();
    // Default timeout on purpose: on Windows a refused connection is only
    // reported after ~2 s of retries, so a shorter timeout would see a timeout.
    let out = testport(&["127.0.0.1", "-p", &port]);

    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("connection refused"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn telnet_mode_sends_input_and_prints_responses() {
    let port = tcp_server().to_string();
    let out = testport_with_input(
        &[
            "127.0.0.1",
            "-p",
            &port,
            "--telnet",
            "-c",
            "1",
            "-w",
            "1000",
        ],
        "hello\nquit\n",
    );
    let text = stdout(&out);

    assert!(out.status.success(), "{text}");
    assert!(text.contains("220 test server"), "{text}");
    assert!(text.contains("ECHO hello"), "{text}");
    assert!(text.contains("bye"), "{text}");
    assert!(text.contains("Connection closed by remote host."), "{text}");
}

// ---------------------------------------------------------------- UDP

#[test]
fn udp_echo_reports_latency_and_banner() {
    let port = udp_echo_server().to_string();
    let out = testport(&[
        "-u",
        "127.0.0.1",
        "-p",
        &port,
        "-b",
        "-c",
        "2",
        "-w",
        "1000",
    ]);
    let text = stdout(&out);

    assert!(out.status.success(), "{text}");
    assert!(text.contains("2/2 probes succeeded"), "{text}");
    assert!(text.contains("Banner:\nECHO testport"), "{text}");
}

#[test]
fn udp_closed_port_fails_with_unreachable() {
    let port = closed_udp_port().to_string();
    let out = testport(&["-u", "127.0.0.1", "-p", &port, "-c", "2", "-w", "1000"]);

    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(
        stdout(&out).contains("port unreachable"),
        "{}",
        stdout(&out)
    );
    assert!(stderr(&out).contains("port closed"), "{}", stderr(&out));
}

#[test]
fn udp_telnet_mode_sends_each_line_as_datagram() {
    let port = udp_echo_server().to_string();
    let out = testport_with_input(
        &[
            "-u",
            "127.0.0.1",
            "-p",
            &port,
            "--telnet",
            "-c",
            "1",
            "-w",
            "500",
        ],
        "one\ntwo\n",
    );
    let text = stdout(&out);

    assert!(out.status.success(), "{text}");
    assert!(text.contains("ECHO one"), "{text}");
    assert!(text.contains("ECHO two"), "{text}");
}

// ---------------------------------------------------------------- errors

#[test]
fn unknown_host_fails() {
    let out = testport(&["no-such-host.invalid", "-p", "80"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("cannot resolve"), "{}", stderr(&out));
}

#[test]
fn version_shows_author() {
    let out = testport(&["-V"]);
    let text = stdout(&out);
    assert!(out.status.success(), "{text}");
    assert!(text.contains(env!("CARGO_PKG_VERSION")), "{text}");
    assert!(
        text.contains("Eugene Butusov <ebutusov@gmail.com>"),
        "{text}"
    );
}

#[test]
fn invalid_arguments_exit_with_code_2() {
    // clap's convention: usage errors exit with 2, distinct from our 1
    // ("target unreachable"), so scripts can tell the two apart.
    let out = testport(&["-t", "-u", "127.0.0.1", "-p", "80"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("cannot be used with"),
        "{}",
        stderr(&out)
    );
}
