//! Bounded stdio transport for the language server (LANG-C-005, DEC-001 §3).
//!
//! `lsp-server`'s stdio reader trusts the `Content-Length` header: a peer
//! sending `Content-Length: 4294967295` makes the dependency attempt a ~4 GB
//! allocation before any byte of payload arrives. The language server is the
//! one place a *stream* of bytes arrives, and the same binary is the
//! reference service surface for agents, so the framing is owned here:
//!
//! - headers are read under a fixed byte budget (`MAX_HEADER_BYTES`), never
//!   buffering more than the budget;
//! - `Content-Length` is validated (digits only, single header, no sign) and
//!   compared with `max_frame` BEFORE the payload buffer is allocated;
//! - the payload is read exactly, validated as UTF-8, and parsed as one
//!   JSON-RPC message;
//! - the inbound channel is bounded, so a flood applies backpressure to the
//!   reader instead of growing the server's memory;
//! - the drain-and-coalesce step below keeps only the newest
//!   `textDocument/didChange` per document in a batch, so a fast typist (or
//!   a hostile peer) cannot queue an unbounded backlog of superseded edits.

use std::io::{self, BufRead, Write};

use crossbeam_channel::{bounded, Receiver, Sender};
use lsp_server::{Connection, Message};

/// Maximum accepted JSON-RPC frame body, in bytes (16 MiB). A frame is the
/// encoded message: the largest legitimate one this server produces or
/// consumes carries a document near the 4 MiB parse cap plus JSON escaping,
/// so 16 MiB leaves room without licensing a multi-gigabyte allocation.
pub const DEFAULT_MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// Maximum accepted header block, in bytes (8 KiB). Real LSP headers are
/// tens of bytes; the cap bounds a header flood without touching payloads.
pub const MAX_HEADER_BYTES: usize = 8 * 1024;

/// Inbound queue capacity (messages). Bounded so a client that outruns the
/// server blocks on its own pipe (backpressure) instead of growing memory.
/// The aggregate bound is capacity × the frame cap: 4 × 16 MiB = 64 MiB,
/// the ceiling a slow analysis can retain. (A 64-message queue licensed
/// ~1 GiB of frame data; the message count alone did not cap bytes.)
pub const INBOUND_CAPACITY: usize = 4;

/// Outbound queue capacity (messages). The writer drains it continuously;
/// a bounded queue keeps a server that produces faster than the client
/// reads from growing without limit (the send blocks instead). Same
/// aggregate bound as inbound: 4 × 16 MiB = 64 MiB.
pub const OUTBOUND_CAPACITY: usize = 4;

/// Largest batch the server drains in one loop iteration (messages).
///
/// Equal to the queue capacity: the reader refills the channel as the
/// server drains it, so a larger batch would multiply the byte bound
/// (16 frames of 16 MiB each is ~256 MiB in one batch while the queue
/// claim says 64 MiB). Four messages still coalesce a superseded edit.
pub const MAX_BATCH: usize = INBOUND_CAPACITY;

/// A framing or payload violation. The stream cannot be resynchronised after
/// one of these (the reader has no way to know where the next frame starts),
/// so the server reports it and exits cleanly rather than continuing on a
/// desynchronised pipe.
#[derive(Debug)]
pub enum TransportError {
    Io(io::Error),
    /// `Content-Length` exceeded the configured cap; the payload was never
    /// allocated.
    Oversize {
        declared: u128,
        max: usize,
    },
    /// Header block exceeded `MAX_HEADER_BYTES`.
    HeaderTooLarge,
    /// Malformed or missing framing header.
    Header(String),
    /// Frame body was not valid UTF-8 or not a JSON-RPC message.
    Payload(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::Io(e) => write!(f, "transport i/o error: {e}"),
            TransportError::Oversize { declared, max } => write!(
                f,
                "frame of {declared} bytes exceeds the {max}-byte limit; rejected before allocation"
            ),
            TransportError::HeaderTooLarge => {
                write!(f, "header block exceeds the {MAX_HEADER_BYTES}-byte limit")
            }
            TransportError::Header(message) => write!(f, "malformed framing header: {message}"),
            TransportError::Payload(message) => write!(f, "malformed payload: {message}"),
        }
    }
}

impl std::error::Error for TransportError {}

impl From<io::Error> for TransportError {
    fn from(e: io::Error) -> Self {
        TransportError::Io(e)
    }
}

/// Read one `Content-Length`-framed JSON-RPC message, refusing to allocate a
/// payload larger than `max_frame` bytes. `Ok(None)` is a clean end of
/// stream between frames.
pub fn read_message<R: BufRead>(
    reader: &mut R,
    max_frame: usize,
) -> Result<Option<Message>, TransportError> {
    let mut budget = MAX_HEADER_BYTES;
    let mut declared: Option<usize> = None;
    let mut saw_header = false;
    loop {
        let Some(line) = read_header_line(reader, &mut budget)? else {
            // End of stream before a complete header block.
            return if saw_header {
                Err(TransportError::Header(
                    "stream ended inside the header block".into(),
                ))
            } else {
                Ok(None)
            };
        };
        if line.is_empty() {
            break;
        }
        saw_header = true;
        let Some((name, value)) = line.split_once(':') else {
            return Err(TransportError::Header(format!("missing colon in {line:?}")));
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            if declared.is_some() {
                return Err(TransportError::Header(
                    "duplicate Content-Length header".into(),
                ));
            }
            let value = value.trim();
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(TransportError::Header(format!(
                    "Content-Length {value:?} is not a decimal number"
                )));
            }
            // Parse with saturation: an absurd digit string must not wrap.
            let parsed: u128 = value.parse().unwrap_or(u128::MAX);
            if parsed > max_frame as u128 {
                return Err(TransportError::Oversize {
                    declared: parsed,
                    max: max_frame,
                });
            }
            declared = Some(parsed as usize);
        }
    }
    let Some(len) = declared else {
        return Err(TransportError::Header("no Content-Length header".into()));
    };
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    let text = String::from_utf8(body)
        .map_err(|e| TransportError::Payload(format!("frame is not UTF-8: {e}")))?;
    let message: Message = serde_json::from_str(&text)
        .map_err(|e| TransportError::Payload(format!("not a JSON-RPC message: {e}")))?;
    Ok(Some(message))
}

/// Read one CRLF-terminated header line, charging it against `budget`.
/// Returns `Ok(None)` at a clean end of stream. A line that would exceed the
/// remaining budget is refused without reading further.
fn read_header_line<R: BufRead>(
    reader: &mut R,
    budget: &mut usize,
) -> Result<Option<String>, TransportError> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err(TransportError::Header(
                    "stream ended inside a header line".into(),
                ))
            };
        }
        let (chunk, consumed) = match available.iter().position(|b| *b == b'\n') {
            Some(idx) => (&available[..=idx], idx + 1),
            None => (available, available.len()),
        };
        if chunk.len() > *budget {
            return Err(TransportError::HeaderTooLarge);
        }
        *budget -= chunk.len();
        line.extend_from_slice(chunk);
        reader.consume(consumed);
        if line.last() == Some(&b'\n') {
            break;
        }
    }
    if line.len() < 2 || !line.ends_with(b"\r\n") {
        return Err(TransportError::Header("line does not end with CRLF".into()));
    }
    line.truncate(line.len() - 2);
    String::from_utf8(line)
        .map(Some)
        .map_err(|e| TransportError::Header(format!("header is not ASCII: {e}")))
}

/// Write one framed message (`lsp-server` owns the canonical encoding).
/// The outbound frame is capped at [`DEFAULT_MAX_FRAME_BYTES`] so a huge
/// result cannot leave the process (or sit in the outbound queue) at an
/// unbounded size. Callers that can produce a very large RESULT should
/// expect [`writer_loop`] to replace it with a protocol error.
pub fn write_message<W: Write>(out: &mut W, message: &Message) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(io::Error::other)?;
    if body.len() > DEFAULT_MAX_FRAME_BYTES {
        return Err(io::Error::other(format!(
            "outbound frame of {} bytes exceeds the {DEFAULT_MAX_FRAME_BYTES}-byte cap",
            body.len()
        )));
    }
    write_body(out, &body)
}

/// Write one pre-serialized frame body (header, bytes, flush).
fn write_body<W: Write>(out: &mut W, body: &[u8]) -> io::Result<()> {
    write!(out, "Content-Length: {}\r\n\r\n", body.len())?;
    out.write_all(body)?;
    out.flush()
}

/// Reader thread body: frame messages from `reader` into `sender` until the
/// stream ends or a violation stops it.
pub fn reader_loop<R: BufRead>(
    mut reader: R,
    sender: &Sender<Message>,
    max_frame: usize,
) -> Result<(), TransportError> {
    loop {
        match read_message(&mut reader, max_frame) {
            Ok(Some(message)) => {
                // The `exit` notification ends the stream: the reader must
                // stop even though the client keeps its pipe open, or the
                // process would wait on stdin forever after shutdown (the
                // reference transport breaks the loop the same way).
                let exit = is_exit(&message);
                // A bounded channel blocks here while the server is busy:
                // that is the backpressure, applied at the transport.
                if sender.send(message).is_err() {
                    return Ok(());
                }
                if exit {
                    return Ok(());
                }
            }
            Ok(None) => return Ok(()),
            Err(e) => return Err(e),
        }
    }
}

/// Writer thread body: drain outgoing messages until the server drops its
/// sender. A response that exceeds the frame cap is replaced with a
/// protocol error (same request id); an oversized notification is logged
/// and dropped. Either way the process never emits an unbounded frame.
pub fn writer_loop<W: Write>(mut out: W, receiver: Receiver<Message>) -> io::Result<()> {
    while let Ok(message) = receiver.recv() {
        let body = serde_json::to_vec(&message).map_err(io::Error::other)?;
        if body.len() <= DEFAULT_MAX_FRAME_BYTES {
            write_body(&mut out, &body)?;
            continue;
        }
        match message {
            Message::Response(response) => {
                eprintln!(
                    "[active-lsp] response of {} bytes exceeds the frame cap; sending an error",
                    body.len()
                );
                let fallback = Message::Response(lsp_server::Response {
                    id: response.id,
                    result: None,
                    error: Some(lsp_server::ResponseError {
                        // JSON-RPC internal error.
                        code: -32603,
                        message: format!(
                            "result exceeds the {DEFAULT_MAX_FRAME_BYTES}-byte frame cap; \
                             narrow the request"
                        ),
                        data: None,
                    }),
                });
                write_message(&mut out, &fallback)?;
            }
            Message::Notification(note) => {
                eprintln!(
                    "[active-lsp] notification `{}` of {} bytes exceeds the frame cap; dropped",
                    note.method,
                    body.len()
                );
            }
            Message::Request(request) => {
                return Err(io::Error::other(format!(
                    "outbound request `{}` of {} bytes exceeds the frame cap",
                    request.method,
                    body.len()
                )));
            }
        }
    }
    Ok(())
}

/// Build a connection over stdin/stdout with the bounded framing above.
/// The returned joins are the transport threads; `shutdown_grace` bounds how
/// long the writer is given to flush after the server stops.
pub fn spawn_stdio(
    max_frame: usize,
) -> (
    Connection,
    std::thread::JoinHandle<Result<(), TransportError>>,
) {
    let (in_tx, in_rx) = bounded::<Message>(INBOUND_CAPACITY);
    let (out_tx, out_rx) = bounded::<Message>(OUTBOUND_CAPACITY);
    let reader = std::thread::spawn(move || -> Result<(), TransportError> {
        let stdin = io::stdin();
        let locked = stdin.lock();
        reader_loop(locked, &in_tx, max_frame)
    });
    std::thread::spawn(move || {
        let stdout = io::stdout();
        let mut locked = stdout.lock();
        if let Err(e) = writer_loop(&mut locked, out_rx) {
            eprintln!("[active-lsp] transport write failed: {e}");
        }
    });
    (
        Connection {
            sender: out_tx,
            receiver: in_rx,
        },
        reader,
    )
}

/// Drain everything already queued, then keep only the newest
/// `textDocument/didChange` per document.
///
/// Coalescing is safe because full-sync changes carry the whole buffer: an
/// older change for the same document is superseded. Requests, responses,
/// and every other notification keep their order; only superseded edits are
/// dropped, and the newest edit keeps the position of the newest one.
pub fn coalesce_batch(batch: &mut Vec<Message>) {
    use std::collections::HashMap;
    let mut newest: HashMap<String, usize> = HashMap::new();
    for (index, message) in batch.iter().enumerate() {
        if let Message::Notification(note) = message {
            if note.method == "textDocument/didChange" {
                if let Some(uri) = note
                    .params
                    .get("textDocument")
                    .and_then(|td| td.get("uri"))
                    .and_then(|u| u.as_str())
                {
                    newest.insert(uri.to_string(), index);
                }
            }
        }
    }
    if newest.is_empty() {
        return;
    }
    let mut index = 0usize;
    batch.retain(|message| {
        let keep = match message {
            Message::Notification(note) if note.method == "textDocument/didChange" => note
                .params
                .get("textDocument")
                .and_then(|td| td.get("uri"))
                .and_then(|u| u.as_str())
                .map(|uri| newest.get(uri) == Some(&index))
                .unwrap_or(true),
            _ => true,
        };
        index += 1;
        keep
    });
}

/// One server-loop iteration: the next message plus everything already
/// queued, coalesced. `Closed` means the reader thread stopped (end of
/// stream or a transport violation), so the loop must end.
pub enum Batch {
    Messages(Vec<Message>),
    Closed,
}

/// Receive the next batch: one blocking receive, then everything already
/// queued. Used by the server loop so coalescing sees the whole backlog.
pub fn next_batch(receiver: &Receiver<Message>) -> Batch {
    let mut batch = Vec::new();
    match receiver.recv() {
        Ok(message) => batch.push(message),
        Err(_) => return Batch::Closed,
    }
    while batch.len() < MAX_BATCH {
        match receiver.try_recv() {
            Ok(message) => batch.push(message),
            Err(_) => break,
        }
    }
    coalesce_batch(&mut batch);
    Batch::Messages(batch)
}

/// True when `message` is an LSP `exit` notification (the server must stop
/// even if a shutdown request never arrived).
pub fn is_exit(message: &Message) -> bool {
    matches!(message, Message::Notification(note) if note.method == "exit")
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_server::{Notification, Request, RequestId};

    fn frame(body: &str) -> Vec<u8> {
        format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes()
    }

    #[test]
    fn queue_capacity_bounds_aggregate_frame_bytes() {
        // The message-count cap is only meaningful together with the frame
        // cap: a slow analysis must not retain a gigabyte of queued frames.
        const {
            assert!(
                INBOUND_CAPACITY * DEFAULT_MAX_FRAME_BYTES <= 64 * 1024 * 1024,
                "inbound queue licenses more than 64 MiB"
            )
        };
        const {
            assert!(
                OUTBOUND_CAPACITY * DEFAULT_MAX_FRAME_BYTES <= 64 * 1024 * 1024,
                "outbound queue licenses more than 64 MiB"
            )
        };
        // A batch drains refilled messages too: it must not multiply the
        // queue's byte bound.
        const {
            assert!(
                MAX_BATCH * DEFAULT_MAX_FRAME_BYTES <= 64 * 1024 * 1024,
                "one batch licenses more than 64 MiB"
            )
        };
    }

    #[test]
    fn an_oversized_response_is_replaced_with_an_error() {
        let (tx, rx) = bounded::<Message>(2);
        let big = "x".repeat(DEFAULT_MAX_FRAME_BYTES + 1024);
        tx.send(Message::Response(lsp_server::Response {
            id: RequestId::from(7),
            result: Some(serde_json::json!({ "value": big })),
            error: None,
        }))
        .expect("send");
        drop(tx);
        let mut out = Vec::new();
        writer_loop(&mut out, rx).expect("writer");
        let mut cursor = std::io::Cursor::new(out);
        let first = read_message(&mut cursor, DEFAULT_MAX_FRAME_BYTES)
            .expect("read")
            .expect("a frame");
        match first {
            Message::Response(response) => {
                assert_eq!(response.id, RequestId::from(7));
                assert!(response.error.is_some(), "the result must be replaced");
                assert!(response.result.is_none());
            }
            other => panic!("expected a response, got {other:?}"),
        }
    }

    fn request() -> String {
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#.to_string()
    }

    #[test]
    fn reads_a_valid_frame() {
        let bytes = frame(&request());
        let mut cursor = io::Cursor::new(bytes);
        let message = read_message(&mut cursor, DEFAULT_MAX_FRAME_BYTES)
            .expect("valid frame")
            .expect("message");
        assert!(matches!(message, Message::Request(_)));
    }

    #[test]
    fn rejects_an_oversize_content_length_before_allocation() {
        // 4 GiB declared, no body: the cap must fire from the header alone.
        let bytes = b"Content-Length: 4294967295\r\n\r\n".to_vec();
        let mut cursor = io::Cursor::new(bytes);
        let err = read_message(&mut cursor, 1024).expect_err("oversize must fail");
        assert!(
            matches!(err, TransportError::Oversize { declared, max } if declared == 4_294_967_295 && max == 1024),
            "{err:?}"
        );
        // Nothing was read into the payload buffer: the cursor sits right
        // after the offending header line (the terminating blank line was
        // never consumed because the cap fired first).
        assert_eq!(
            cursor.position(),
            b"Content-Length: 4294967295\r\n".len() as u64
        );
    }

    #[test]
    fn rejects_signed_duplicate_and_missing_length_headers() {
        for (bytes, label) in [
            (b"Content-Length: +5\r\n\r\n".to_vec(), "signed"),
            (
                b"Content-Length: 5\r\nContent-Length: 5\r\n\r\n".to_vec(),
                "duplicate",
            ),
            (b"Content-Type: text/x\r\n\r\n".to_vec(), "missing"),
        ] {
            let mut cursor = io::Cursor::new(bytes);
            let err = read_message(&mut cursor, 1024).expect_err(label);
            assert!(matches!(err, TransportError::Header(_)), "{label}: {err:?}");
        }
    }

    #[test]
    fn rejects_an_oversize_header_block() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"Content-Length: 5\r\n");
        bytes.extend_from_slice(&vec![b'X'; MAX_HEADER_BYTES + 16]);
        bytes.extend_from_slice(b"\r\n\r\n");
        let mut cursor = io::Cursor::new(bytes);
        let err = read_message(&mut cursor, 1024).expect_err("header cap");
        assert!(matches!(err, TransportError::HeaderTooLarge), "{err:?}");
    }

    #[test]
    fn rejects_invalid_utf8_and_non_json_payloads() {
        let mut bytes = b"Content-Length: 2\r\n\r\n".to_vec();
        bytes.extend_from_slice(&[0xff, 0xfe]);
        let err = read_message(&mut io::Cursor::new(bytes), 1024).expect_err("utf8");
        assert!(matches!(err, TransportError::Payload(_)), "{err:?}");

        let bytes = frame("not json");
        let err = read_message(&mut io::Cursor::new(bytes), 1024).expect_err("json");
        assert!(matches!(err, TransportError::Payload(_)), "{err:?}");
    }

    #[test]
    fn a_clean_end_of_stream_is_not_an_error() {
        let mut cursor = io::Cursor::new(Vec::new());
        assert!(read_message(&mut cursor, 1024)
            .expect("clean eof")
            .is_none());
    }

    fn change(uri: &str, version: i32) -> Message {
        Message::Notification(Notification {
            method: "textDocument/didChange".into(),
            params: serde_json::json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": format!("x = {version}\n")}],
            }),
        })
    }

    fn open(uri: &str) -> Message {
        Message::Notification(Notification {
            method: "textDocument/didOpen".into(),
            params: serde_json::json!({
                "textDocument": {"uri": uri, "languageId": "ACTIVE", "version": 1, "text": "x = 1\n"},
            }),
        })
    }

    fn version_of(message: &Message) -> i64 {
        let Message::Notification(note) = message else {
            panic!("notification")
        };
        note.params["textDocument"]["version"]
            .as_i64()
            .expect("version")
    }

    #[test]
    fn a_burst_is_drained_in_bounded_batches() {
        // Review finding on LANG-C-005: an unbounded batch could hold 64
        // frames of up to 16 MiB each at once. The drain stops at
        // MAX_BATCH; the rest waits in the bounded queue.
        let (tx, rx) = bounded::<Message>(128);
        for i in 0..(MAX_BATCH * 2) {
            tx.send(open(&format!("file:///burst-{i}.active")))
                .expect("queue has room");
        }
        match next_batch(&rx) {
            Batch::Messages(batch) => assert!(
                batch.len() <= MAX_BATCH,
                "batch must be bounded, got {}",
                batch.len()
            ),
            Batch::Closed => panic!("open receiver cannot be closed"),
        }
    }

    #[test]
    fn coalescing_keeps_only_the_newest_change_per_document() {
        let mut batch = vec![
            open("file:///a.active"),
            change("file:///a.active", 2),
            change("file:///b.active", 3),
            change("file:///a.active", 4),
            Message::Request(Request {
                id: RequestId::from(7),
                method: "textDocument/hover".into(),
                params: serde_json::Value::Null,
            }),
            change("file:///a.active", 5),
        ];
        coalesce_batch(&mut batch);
        assert_eq!(batch.len(), 4, "{batch:?}");
        assert!(
            matches!(&batch[0], Message::Notification(n) if n.method == "textDocument/didOpen")
        );
        assert_eq!(version_of(&batch[1]), 3, "b's only change survives");
        assert!(
            matches!(&batch[2], Message::Request(_)),
            "requests keep order"
        );
        assert_eq!(version_of(&batch[3]), 5, "newest a change wins");
    }

    #[test]
    fn a_batch_of_only_changes_collapses_to_one_per_document() {
        let mut batch = vec![
            change("file:///a.active", 1),
            change("file:///a.active", 2),
            change("file:///a.active", 3),
        ];
        coalesce_batch(&mut batch);
        assert_eq!(batch.len(), 1);
        assert_eq!(version_of(&batch[0]), 3);
    }
}
