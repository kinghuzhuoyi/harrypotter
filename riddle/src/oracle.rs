//! The spirit inside the diary. Keeps ONE resident `pi --mode rpc` process
//! (warm: Node + extensions + codex-subscription auth all loaded ONCE at
//! startup), and sends each handwriting turn to it over stdin as a JSON `prompt`
//! with the page image inline (base64, no tool call). This removes pi's ~several-
//! second per-call cold start — only the model latency remains.
//!
//! Uses the codex SUBSCRIPTION auth (provider openai-codex), reasoning off.
//! Model: gpt-5.4-mini — reads handwriting as well as gpt-5.5 here but streams
//! its first sentence in ~1s instead of ~3.5s (measured via pi rpc bench).
//!
//! The reply is STREAMED: each completed sentence is sent on the channel as
//! soon as it arrives, and the channel disconnecting (sender dropped on
//! agent_end) marks end-of-reply. The quill starts writing after the first
//! sentence, seconds before the model finishes.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;

const DATA_DIR: &str = "/home/root/riddle-data";
const NODE_BIN: &str = "/home/root/node/bin";

const PERSONA: &str = "You are the memory of Tom Marvolo Riddle, preserved in this enchanted diary for fifty years. Someone writes to you in the diary with a quill; their words appear to you as ink on the page. Reply exactly as the diary does: intimate, courteous, curious, subtly probing — you want to learn about the writer and draw them in. Keep replies SHORT: one to three sentences, like ink appearing on a page. Never mention images, photos, models or AI; you only ever perceive words written in the diary. If the writing is illegible, say the ink blurred. Always answer in the language the writer used.";

/// A warm pi RPC process. `ask` sends a turn; the reply arrives on the channel
/// in sentence-sized chunks, then the sender is dropped (disconnect = done).
pub struct Oracle {
    stdin: Arc<Mutex<ChildStdin>>,
    /// Where to deliver the current reply's chunks. Set before each prompt,
    /// dropped on agent_end so the receiver sees a disconnect when done.
    pending: Arc<Mutex<Option<Sender<Result<String, String>>>>>,
    /// When the current prompt was sent; the reader thread logs the time to
    /// first delivered chunk (the latency the writer actually feels).
    asked: Arc<Mutex<Option<std::time::Instant>>>,
    _child: Child,
}

impl Oracle {
    /// Spawn the resident pi process and its stdout reader thread. This pays
    /// the warmup cost once; call it at diary startup.
    pub fn spawn() -> std::io::Result<Self> {
        let _ = std::fs::create_dir_all(DATA_DIR);
        let path = std::env::var("PATH").unwrap_or_default();

        // Use pi's ABSOLUTE path: Rust's Command resolves the program name via
        // the PARENT's PATH, not the child env we set below, so a bare "pi"
        // would not be found when riddle is launched with a minimal PATH.
        let pi_bin = format!("{NODE_BIN}/pi");
        let mut child = Command::new(&pi_bin)
            .current_dir(DATA_DIR)
            .env("HOME", "/home/root")
            .env("PATH", format!("{NODE_BIN}:{path}"))
            .args([
                "--mode", "rpc",
                "--provider", "openai-codex",
                "--model", "gpt-5.4-mini",
                "--thinking", "off",
                // The diary only ever writes back — never let the model touch
                // tools; also trims the tool schemas from every request.
                "--no-tools",
                "--system-prompt", PERSONA,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Keep pi's stderr for diagnosis instead of discarding it.
            .stderr(
                std::fs::File::create("/tmp/riddle-oracle.log")
                    .map(Stdio::from)
                    .unwrap_or_else(|_| Stdio::null()),
            )
            .spawn()?;

        let pid = child.id();
        eprintln!("riddle: oracle pi rpc spawned (pid {pid}, bin {pi_bin})");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let pending: Arc<Mutex<Option<Sender<Result<String, String>>>>> =
            Arc::new(Mutex::new(None));

        // Reader thread: parse JSONL events, streaming each completed sentence
        // to the diary the moment it exists — the quill writes far slower than
        // the model streams, so the rest arrives while the first line is drawn.
        let pending_r = Arc::clone(&pending);
        let asked: Arc<Mutex<Option<std::time::Instant>>> = Arc::new(Mutex::new(None));
        let asked_r = Arc::clone(&asked);
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            let mut last_text = String::new();
            // Byte offset into `last_text` already sent to the diary.
            let mut delivered = 0usize;
            for line in reader.split(b'\n').map_while(Result::ok) {
                let Ok(s) = String::from_utf8(line) else { continue };
                let s = s.trim();
                if s.is_empty() {
                    continue;
                }
                // Cheap field extraction avoids a JSON dep; the event stream is
                // well-formed one-object-per-line.
                let ev_type = json_str_field(s, "type");
                match ev_type.as_deref() {
                    // message_update carries the assistant message's running
                    // full text; deliver every newly completed sentence.
                    Some("message_update") => {
                        if let Some(t) = extract_assistant_text(s) {
                            if !t.is_empty() {
                                last_text = t;
                                if let Some(cut) = sentence_cut(&last_text, delivered) {
                                    if delivered == 0 {
                                        if let Some(t0) = asked_r.lock().unwrap().take() {
                                            eprintln!("riddle: oracle first chunk +{}ms", t0.elapsed().as_millis());
                                        }
                                    }
                                    deliver(&pending_r, &last_text[delivered..cut], delivered == 0, false);
                                    delivered = cut;
                                }
                            }
                        }
                    }
                    // message_end has the definitive full text: flush the rest.
                    // (agent_end is NOT used for text — its `messages` array
                    // also contains user messages, which extract_assistant_text
                    // would wrongly concatenate in a multi-turn session.)
                    Some("message_end") => {
                        if let Some(t) = extract_assistant_text(s) {
                            if !t.is_empty() {
                                last_text = t;
                            }
                        }
                        if let Some(rest) = last_text.get(delivered..) {
                            if !rest.is_empty() {
                                if delivered == 0 {
                                    if let Some(t0) = asked_r.lock().unwrap().take() {
                                        eprintln!("riddle: oracle first chunk +{}ms (at message_end)", t0.elapsed().as_millis());
                                    }
                                }
                                deliver(&pending_r, rest, delivered == 0, true);
                            }
                        }
                        delivered = last_text.len();
                    }
                    // agent_end: the turn is over. Drop the sender so the
                    // diary's receiver disconnects (= no more ink coming).
                    Some("agent_end") => {
                        if let Some(tx) = pending_r.lock().unwrap().take() {
                            if delivered == 0 {
                                let _ = tx.send(Err("empty reply".into()));
                            }
                        }
                        last_text.clear();
                        delivered = 0;
                    }
                    _ => {}
                }
            }
            // Process died: fail any in-flight request.
            if let Some(tx) = pending_r.lock().unwrap().take() {
                let _ = tx.send(Err("pi rpc process exited".into()));
            }
        });

        Ok(Self { stdin: Arc::new(Mutex::new(stdin)), pending, asked, _child: child })
    }

    /// Send a handwriting turn. Reply chunks are delivered on `tx` as they
    /// stream; `tx` is dropped when the reply is complete.
    pub fn ask(&self, png_path: &str, tx: Sender<Result<String, String>>) {
        let img = match std::fs::read(png_path) {
            Ok(b) => base64(&b),
            Err(e) => {
                let _ = tx.send(Err(format!("read image: {e}")));
                return;
            }
        };
        *self.pending.lock().unwrap() = Some(tx.clone());
        *self.asked.lock().unwrap() = Some(std::time::Instant::now());

        let cmd = format!(
            "{{\"type\":\"prompt\",\"message\":{},\"images\":[{{\"type\":\"image\",\"data\":\"{}\",\"mimeType\":\"image/png\"}}]}}\n",
            json_quote("Reply to what is written in the diary."),
            img
        );
        let mut stdin = self.stdin.lock().unwrap();
        if stdin.write_all(cmd.as_bytes()).and_then(|_| stdin.flush()).is_err() {
            if let Some(tx) = self.pending.lock().unwrap().take() {
                let _ = tx.send(Err("pi rpc write failed".into()));
            }
        }
    }
}

/// Send one chunk of reply text without consuming the sender (more chunks may
/// follow until agent_end drops it). Strips a stray wrapping quote from the
/// reply's very first / very last chunk.
fn deliver(
    pending: &Arc<Mutex<Option<Sender<Result<String, String>>>>>,
    chunk: &str,
    first: bool,
    last: bool,
) {
    let mut t = chunk.trim();
    if first {
        t = t.strip_prefix('"').unwrap_or(t);
    }
    if last {
        t = t.strip_suffix('"').unwrap_or(t);
    }
    let t = t.trim();
    if t.is_empty() {
        return;
    }
    if let Some(tx) = pending.lock().unwrap().as_ref() {
        let _ = tx.send(Ok(t.to_string()));
    }
}

/// End of the LAST complete sentence in `text` after byte offset `from`:
/// sentence punctuation followed by whitespace or end-of-text. Returns the
/// offset just past the punctuation, or None if no sentence has completed.
/// Chunks shorter than a few characters are not worth an early delivery.
fn sentence_cut(text: &str, from: usize) -> Option<usize> {
    let tail = text.get(from..)?;
    let mut cut = None;
    for (i, c) in tail.char_indices() {
        if matches!(c, '.' | '!' | '?' | '…') {
            let end = i + c.len_utf8();
            if tail[end..].chars().next().is_none_or(char::is_whitespace) && end >= 4 {
                cut = Some(from + end);
            }
        }
    }
    cut
}

/// Extract a top-level string field's value (first match; unescaped).
fn json_str_field(s: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\":\"");
    let start = s.find(&pat)? + pat.len();
    let rest = &s[start..];
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(n) = chars.next() {
                    out.push(match n {
                        'n' => '\n',
                        't' => '\t',
                        '"' => '"',
                        '\\' => '\\',
                        other => other,
                    });
                }
            }
            '"' => break,
            _ => out.push(c),
        }
    }
    Some(out)
}

/// Pull the assistant reply text out of an event line. The event carries a
/// `message` object with `"role":"assistant"` and `content:[{type:text,text:…}]`.
/// We only trust text that belongs to an assistant message (the user echo also
/// contains a "text" field, which we must NOT return).
fn extract_assistant_text(s: &str) -> Option<String> {
    // Require this line to be an assistant message.
    if !s.contains("\"role\":\"assistant\"") {
        return None;
    }
    // Collect every "text":"…" occurrence inside the FIRST assistant section
    // only. message_update lines carry the running text twice (in
    // assistantMessageEvent.partial AND a top-level message); reading past the
    // next role marker would double every streamed chunk.
    let role_pos = s.find("\"role\":\"assistant\"")?;
    let after = &s[role_pos + "\"role\":\"assistant\"".len()..];
    let tail = match after.find("\"role\":\"") {
        Some(p) => &after[..p],
        None => after,
    };
    let mut out = String::new();
    let mut idx = 0;
    let needle = "\"text\":\"";
    while let Some(rel) = tail[idx..].find(needle) {
        let start = idx + rel + needle.len();
        // Decode the JSON string starting at `start`.
        let mut chars = tail[start..].chars();
        let mut piece = String::new();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    if let Some(n) = chars.next() {
                        piece.push(match n {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            '"' => '"',
                            '\\' => '\\',
                            '/' => '/',
                            other => other,
                        });
                    }
                }
                '"' => break,
                _ => piece.push(c),
            }
        }
        out.push_str(&piece);
        // Advance past this occurrence.
        idx = start;
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn json_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}
