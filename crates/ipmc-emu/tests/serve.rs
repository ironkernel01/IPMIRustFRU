// serve.rs - the built binary in serve mode: a client over TCP sends Get Device ID, enables
// IPMB-B, breaks IPMB-A and checks that A requests get no answer while B still works.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::Duration;

/// SILENCE_MS -- how long a request is given before "no answer" is concluded; well over the
/// 1 ms transmission time of the emulator.
const SILENCE_MS: u64 = 300;
/// ANSWER_MS -- most time one expected line may take.
const ANSWER_MS: u64 = 5000;

// Requests from 20h to 82h, checksums included (netfn 06h cmd 01h; netfn 2Ch cmd 09h Set IPMB
// State: VSO id 03h, IPMB-A no change FFh, IPMB-B local control 01h).
const GET_DEVICE_ID_SEQ1: &str = "82 18 66 20 04 01 DB";
const SET_IPMB_STATE_SEQ2: &str = "82 B0 CE 20 08 09 03 FF 01 CC";
const GET_DEVICE_ID_SEQ3: &str = "82 18 66 20 0C 01 D3";
const GET_DEVICE_ID_SEQ4: &str = "82 18 66 20 10 01 CF";

/// Server -- the running `ipmc-emu serve`, killed when dropped.
struct Server {
    child: Child,
    /// Held open so the server's later prints never meet a closed pipe.
    _stdout: BufReader<ChildStdout>,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// start -- runs `serve` on a free port with `args`; returns it and the port it printed.
fn start(args: &[&str]) -> (Server, u16) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ipmc-emu"))
        .arg("serve")
        .args(["--port", "0"])
        .args(args)
        .stdout(Stdio::piped())
        .spawn()
        .expect("run ipmc-emu serve");
    let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
    let mut line = String::new();
    stdout.read_line(&mut line).expect("listening line");
    let port = line
        .trim()
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| panic!("no port in {line:?}"));
    (
        Server {
            child,
            _stdout: stdout,
        },
        port,
    )
}

/// Client -- a connected Chassis Manager side.
struct Client {
    reader: BufReader<TcpStream>,
}

impl Client {
    fn connect(port: u16) -> Client {
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream.set_nodelay(true).expect("nodelay");
        Client {
            reader: BufReader::new(stream),
        }
    }

    fn send(&mut self, text: &str) {
        let stream = self.reader.get_mut();
        stream
            .write_all(format!("{text}\n").as_bytes())
            .expect("send");
    }

    /// line_within -- the next line, or None when none arrives within `ms`.
    fn line_within(&mut self, ms: u64) -> Option<String> {
        self.reader
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(ms)))
            .expect("timeout");
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => panic!("server closed the connection"),
            Ok(_) => Some(line.trim().to_string()),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                None
            }
            Err(e) => panic!("read: {e}"),
        }
    }

    fn expect_line(&mut self) -> String {
        self.line_within(ANSWER_MS).expect("a line from the server")
    }
}

#[test]
fn get_device_id_is_answered_and_a_down_bus_loses_requests_while_b_works() {
    let (_server, port) = start(&["--b-fitted", "1"]);
    let mut cm = Client::connect(port);
    assert_eq!(cm.expect_line(), "#ready 82");

    cm.send(&format!("A {GET_DEVICE_ID_SEQ1}"));
    let response = cm.expect_line();
    assert!(response.starts_with("A 20 1C "), "{response}");

    cm.send(&format!("A {SET_IPMB_STATE_SEQ2}"));
    let response = cm.expect_line();
    assert!(response.starts_with("A 20 B4 ") || response.starts_with("B 20 B4 "));

    cm.send("#bus A down");
    cm.send(&format!("A {GET_DEVICE_ID_SEQ3}"));
    assert_eq!(cm.line_within(SILENCE_MS), None, "A is down");

    cm.send(&format!("B {GET_DEVICE_ID_SEQ4}"));
    let response = cm.expect_line();
    assert!(response.starts_with("B 20 1C "), "{response}");

    cm.send("#bus A up");
    cm.send(&format!("A {GET_DEVICE_ID_SEQ3}"));
    let response = cm.expect_line();
    assert!(response.contains(" 20 1C "), "{response}");
}

#[test]
fn a_bad_line_is_answered_with_an_error_and_the_session_continues() {
    let (_server, port) = start(&[]);
    let mut cm = Client::connect(port);
    assert_eq!(cm.expect_line(), "#ready 82");
    cm.send("A 82 XY");
    assert_eq!(cm.expect_line(), "#error A 82 XY: bad hex byte");
    cm.send("hello");
    assert_eq!(cm.expect_line(), "#error hello: unknown message");
    cm.send(&format!("A {GET_DEVICE_ID_SEQ1}"));
    assert!(cm.expect_line().starts_with("A 20 1C "));
}

#[test]
fn a_new_client_gets_a_fresh_board_and_reset_restarts_it() {
    let (_server, port) = start(&["--b-fitted", "1"]);
    {
        let mut first = Client::connect(port);
        assert_eq!(first.expect_line(), "#ready 82");
        first.send(&format!("A {SET_IPMB_STATE_SEQ2}"));
        let _ = first.expect_line();
        first.send("#quit");
        // The server closes the session; the stream ends.
        let mut rest = String::new();
        let _ = first.reader.read_to_string(&mut rest);
    }
    let mut second = Client::connect(port);
    assert_eq!(second.expect_line(), "#ready 82");
    // B is disabled again at power-up: a request on B gets no answer.
    second.send(&format!("B {GET_DEVICE_ID_SEQ4}"));
    assert_eq!(second.line_within(SILENCE_MS), None);
    second.send("#reset");
    assert_eq!(second.expect_line(), "#ready 82");
}
