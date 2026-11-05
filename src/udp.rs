//! UDP testing.
//!
//! UDP is connectionless: there is no handshake, so "connecting" sends nothing
//! over the network. That makes UDP port testing fundamentally different:
//!
//! - We get a **reply**: the port is open, and the round-trip time is our latency.
//! - We get **nothing**: the port may be open (the service ignored our data),
//!   or a firewall dropped the packet. We cannot tell — "open|filtered".
//! - We get an **ICMP "port unreachable"**: the host is up, and the port is closed.

use crate::cli::Args;
use crate::{AppResult, fmt_ms, is_timeout, print_summary};
use std::io::{self, BufRead, Write};
use std::net::{SocketAddr, UdpSocket};
use std::thread;
use std::time::{Duration, Instant};

/// Pause between probes.
const PROBE_INTERVAL: Duration = Duration::from_millis(250);

/// Data sent in each probe. Echo-style services answer it; services with a
/// binary protocol (DNS, NTP...) will usually ignore it — use --telnet mode
/// to send something meaningful to them.
const PROBE_PAYLOAD: &[u8] = b"testport\r\n";

/// Largest possible UDP payload; a smaller buffer would silently truncate
/// bigger datagrams (UDP delivers whole datagrams, never partial ones).
const MAX_DATAGRAM: usize = 65_535;

/// Outcome of one probe.
enum Reply {
    Data(Vec<u8>, Duration),
    NoResponse,
    PortUnreachable,
}

pub fn run(addr: SocketAddr, args: &Args) -> AppResult<()> {
    let timeout = args.timeout();

    // Bind a local socket of the same address family as the target, on
    // port 0 = "let the OS pick a free port". An IPv4 socket can't talk to an
    // IPv6 address (and vice versa), so this choice matters.
    let local = if addr.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(local)?;

    // For UDP, `connect` only records the default destination (so we can use
    // `send`/`recv` instead of `send_to`/`recv_from`) and makes the OS filter
    // out datagrams from other addresses. It also lets the OS report ICMP
    // errors back to us. Nothing is sent on the network here.
    socket.connect(addr)?;
    socket.set_read_timeout(Some(timeout))?;

    println!("UDP is connectionless; only a reply proves the port is open. Measuring times...");

    let mut samples = Vec::new();
    let mut first_reply: Option<Vec<u8>> = None;
    let mut unreachable = false;

    for i in 1..=args.count {
        if i > 1 {
            thread::sleep(PROBE_INTERVAL);
        }
        match probe(&socket)? {
            Reply::Data(data, rtt) => {
                println!("-> {i} {}", fmt_ms(rtt));
                samples.push(rtt);
                // `get_or_insert` keeps only the first reply for the banner.
                first_reply.get_or_insert(data);
            }
            Reply::NoResponse => println!("-> {i} no response within {}", fmt_ms(timeout)),
            Reply::PortUnreachable => {
                println!("-> {i} port unreachable (ICMP)");
                unreachable = true;
            }
        }
    }

    if samples.is_empty() {
        if unreachable {
            return Err("port closed (host replied with ICMP port unreachable)".into());
        }
        // Silence is not a failure we can prove, so we don't return an error.
        println!("Done, no replies: port is open|filtered (UDP gives no way to tell which).");
    } else {
        print_summary(&samples, args.count);
    }

    if args.banner {
        match &first_reply {
            Some(data) => println!("Banner:\n{}", String::from_utf8_lossy(data).trim_end()),
            None => println!("Banner: (no reply received)"),
        }
    }

    if args.telnet {
        session(socket, timeout)?;
    }
    Ok(())
}

/// Sends one probe datagram and waits for the reply.
fn probe(socket: &UdpSocket) -> io::Result<Reply> {
    let mut buf = vec![0u8; MAX_DATAGRAM];
    let start = Instant::now();

    // Wrapping send+recv in a closure lets us classify errors from both calls
    // in one place: on Linux, an ICMP error from an earlier probe may show up
    // on the next `send`, on Windows it shows up on `recv`.
    let result = (|| {
        socket.send(PROBE_PAYLOAD)?;
        socket.recv(&mut buf)
    })();

    match result {
        Ok(n) => Ok(Reply::Data(buf[..n].to_vec(), start.elapsed())),
        Err(e) if is_timeout(&e) => Ok(Reply::NoResponse),
        Err(e) if is_port_unreachable(&e) => Ok(Reply::PortUnreachable),
        Err(e) => Err(e),
    }
}

/// Portability trap #2: an ICMP "port unreachable" on a connected UDP socket
/// surfaces as `ConnectionRefused` on Linux/macOS, but as `ConnectionReset`
/// (WSAECONNRESET) on Windows.
fn is_port_unreachable(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionRefused | io::ErrorKind::ConnectionReset
    )
}

/// Interactive mode: every stdin line is sent as one datagram, and every
/// datagram received is printed.
fn session(socket: UdpSocket, timeout: Duration) -> AppResult<()> {
    socket.set_read_timeout(None)?;
    let receiver = socket.try_clone()?; // second handle to the same OS socket

    println!(
        "Telnet mode (UDP): each line is sent as one datagram; EOF (Ctrl+D, or Ctrl+Z then Enter on Windows) to quit."
    );

    // The receiving thread runs until the process exits. We don't join it:
    // with UDP there is no "connection closed" event to wait for.
    thread::spawn(move || {
        let mut buf = vec![0u8; MAX_DATAGRAM];
        let mut stdout = io::stdout();
        loop {
            match receiver.recv(&mut buf) {
                Ok(n) => {
                    let _ = stdout.write_all(&buf[..n]);
                    let _ = stdout.flush();
                }
                // ICMP errors don't break a UDP "session"; report and continue.
                Err(e) if is_port_unreachable(&e) => println!("[port unreachable]"),
                Err(_) => break,
            }
        }
    });

    // For UDP we end lines with "\n" only, like `nc -u` does.
    for line in io::stdin().lock().lines() {
        let mut datagram = line?;
        datagram.push('\n');
        // An ICMP error from a previous datagram may be reported here; the
        // receiver thread already tells the user, so it's not fatal.
        if let Err(e) = socket.send(datagram.as_bytes())
            && !is_port_unreachable(&e)
        {
            return Err(e.into());
        }
    }

    // stdin closed: give late replies a chance to arrive before exiting.
    thread::sleep(timeout);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Starts a UDP echo server on a free local port and returns its address.
    /// The thread is detached; it dies together with the test process.
    fn echo_server() -> SocketAddr {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        thread::spawn(move || {
            let mut buf = [0u8; 1024];
            while let Ok((n, peer)) = server.recv_from(&mut buf) {
                let _ = server.send_to(&buf[..n], peer);
            }
        });
        addr
    }

    /// A client socket connected to `addr` with a short read timeout.
    fn client(addr: SocketAddr) -> UdpSocket {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.connect(addr).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        socket
    }

    #[test]
    fn unreachable_detection_covers_both_platform_variants() {
        use io::ErrorKind::*;
        assert!(is_port_unreachable(&io::Error::from(ConnectionRefused))); // Linux
        assert!(is_port_unreachable(&io::Error::from(ConnectionReset))); // Windows
        assert!(!is_port_unreachable(&io::Error::from(TimedOut)));
    }

    #[test]
    fn probe_gets_reply_from_echo_server() {
        let socket = client(echo_server());
        match probe(&socket).unwrap() {
            Reply::Data(data, _rtt) => assert_eq!(data, PROBE_PAYLOAD),
            _ => panic!("expected a reply from the echo server"),
        }
    }

    #[test]
    fn probe_reports_silence_as_no_response() {
        // A bound socket that never answers: datagrams arrive, nothing comes back.
        let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
        let socket = client(silent.local_addr().unwrap());
        assert!(matches!(probe(&socket).unwrap(), Reply::NoResponse));
    }

    #[test]
    fn probe_reports_closed_port_as_unreachable() {
        // Bind and immediately drop a socket to get a port that is (almost
        // certainly) closed; the OS then answers with ICMP port unreachable.
        let closed = UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let socket = client(closed);
        assert!(matches!(probe(&socket).unwrap(), Reply::PortUnreachable));
    }
}
