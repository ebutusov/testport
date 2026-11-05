# testport

A small command-line tool for testing network connectivity, written in Rust.

`testport` connects to a port on a host over TCP or UDP and tells you whether
the port is reachable, how long the connection took, and what the round-trip
latency is. It can also print the service's greeting banner, and it has a
telnet-like mode for typing to the remote service and reading its replies.

```
> testport example.com -p 80
Trying example.com port 80...
Resolved example.com to 172.66.147.243
Connected in 13.6ms, measuring times...
-> 1 16.9ms
-> 2 16.6ms
-> 3 15.3ms
-> 4 17.9ms
Done, avg latency is 16.7ms (min 15.3ms, max 17.9ms, 4/4 probes succeeded)
```

## Building

You need a Rust toolchain ([rustup.rs](https://rustup.rs)).

```sh
cargo build --release
```

The binary is written to `target/release/testport` (`testport.exe` on
Windows). You can copy it anywhere on your `PATH`, or rename it to something
shorter such as `tp`.

## Usage

```
testport [OPTIONS] --port <PORT> <HOST>
```

| Option | Description |
| --- | --- |
| `<HOST>` | Host name or IP address to test |
| `-p, --port <PORT>` | Port to connect to (1–65535) |
| `-t, --tcp` | Use TCP (the default) |
| `-u, --udp` | Use UDP (cannot be combined with `-t`) |
| `-b, --banner` | Print the banner (first data) sent by the service |
| `--telnet` | Stay connected and send what you type to the remote host |
| `-c, --count <N>` | Number of latency probes (default 4) |
| `-w, --timeout <MS>` | Timeout for connecting or waiting for a reply, in milliseconds (default 3000) |
| `-h, --help` | Show help |
| `-V, --version` | Show version and author |

### Examples

Check that a web server is reachable:

```sh
testport example.com -p 443
```

Read an SSH or mail server's greeting:

```sh
testport mail.example.com -p 25 -b
```

Talk to a service interactively:

```sh
testport example.com -p 80 --telnet
```

Then type, for example, `HEAD / HTTP/1.0`, `Host: example.com` and an empty
line. End the session with Ctrl+D (Linux/macOS) or Ctrl+Z then Enter
(Windows).

Send a request from a script by piping it in:

```sh
printf 'HEAD / HTTP/1.0\nHost: example.com\n\n' | testport example.com -p 80 --telnet
```

Test a UDP service with more probes and a shorter timeout:

```sh
testport -u 192.168.1.1 -p 53 -c 10 -w 500
```

## How it works

**TCP.** Each latency probe opens a new connection and closes it again. The
time it takes to connect is one network round trip (the TCP handshake), so the
measurement works on any open port, whatever service is listening.

**UDP.** UDP has no connection step, so `testport` sends a small packet
(`testport\r\n`) and waits for a reply. There are three possible results:

| Result | Meaning |
| --- | --- |
| A reply arrives | The port is open; the round trip is the latency |
| No reply within the timeout | Open or filtered: the service may have ignored the packet, or a firewall dropped it. UDP gives no way to tell which |
| "Port unreachable" from the host | The port is closed |

Many UDP services (DNS, NTP and so on) only answer requests in their own
format, so they will show as "open or filtered". Use `--telnet` to send them
your own data.

**Banner.** Some services (SSH, SMTP, FTP) send a greeting as soon as you
connect; `-b` prints it. Others, such as HTTP, wait for the client to speak
first, so no banner is shown for them.

**Telnet mode.** This is a plain text session, like `telnet host port` is
typically used for testing: it does not perform the telnet protocol's option
negotiation. Over TCP, each line is sent with a `\r\n` ending, which most text
protocols expect. Over UDP, each line is sent as one packet ending in `\n`.
When input ends, `testport` waits up to the timeout for any final reply, then
exits.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success (for UDP, also "open or filtered") |
| 1 | The target could not be reached (closed, timed out, or unknown host) |
| 2 | Invalid command-line arguments |

This makes `testport` usable in scripts:

```sh
testport db.internal -p 5432 -c 1 && ./run-migrations.sh
```

## Notes

- On Windows, a connection to a closed TCP port is only reported as refused
  after about 2 seconds, because Windows retries refused connections. With a
  timeout shorter than that, closed ports show up as timeouts.
- When a host name resolves to both IPv4 and IPv6 addresses, IPv4 is used.

## Development

```sh
cargo test     # unit and integration tests, all against local servers
cargo clippy   # lints
cargo fmt      # formatting
```

The code is commented to explain Rust and network-programming practices along
the way. It is a good starting point if you are learning either.

| File | Contents |
| --- | --- |
| `src/main.rs` | Entry point, host name lookup, shared helpers |
| `src/cli.rs` | Command-line options |
| `src/tcp.rs` | TCP probes, banner and telnet mode |
| `src/udp.rs` | UDP probes and telnet mode |
| `tests/cli.rs` | Integration tests that run the real binary |

## License

MIT. See [LICENSE](LICENSE).

Copyright (c) 2026 Eugene Butusov <ebutusov@gmail.com>
