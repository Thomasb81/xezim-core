//! Buffered stdout sink for `$display`/`$write` output.
//!
//! Two modes:
//!   * Inline   — buffered `BufWriter<Stdout>` on the caller thread. Still a
//!                win over bare `print!`, which goes through a `LineWriter`
//!                and syscalls on every `\n` — picorv32 emits ~8k single-byte
//!                writes via `$write("%c",...)` and each was its own lock
//!                acquisition on the global stdout.
//!   * Threaded — hands filled buffers to a dedicated writer thread via an
//!                mpsc channel. Single-producer FIFO → output ordering is
//!                preserved.

use std::io::{self, BufWriter, Stdout, Write};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

const BUF_CAPACITY: usize = 16 * 1024;
const FLUSH_THRESHOLD: usize = 8 * 1024;

enum Mode {
    Inline(BufWriter<Stdout>),
    Threaded {
        buf: Vec<u8>,
        tx: Option<Sender<Msg>>,
        handle: Option<JoinHandle<()>>,
    },
}

enum Msg {
    Chunk(Vec<u8>),
    FlushChunk(Vec<u8>),
    Shutdown,
}

pub struct StdoutSink {
    mode: Mode,
}

fn run_threaded_writer<W: Write>(rx: Receiver<Msg>, mut writer: W) {
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Chunk(bytes) => {
                let _ = writer.write_all(&bytes);
            }
            Msg::FlushChunk(bytes) => {
                let _ = writer.write_all(&bytes);
                // writeln_str() requests this path so `$display` stays visible
                // during a long-running or stalled simulation. Plain `$write`
                // chunks retain the worker-side batching optimization above.
                let _ = writer.flush();
            }
            Msg::Shutdown => break,
        }
    }
    let _ = writer.flush();
}

impl StdoutSink {
    pub fn inline() -> Self {
        StdoutSink { mode: Mode::Inline(BufWriter::with_capacity(BUF_CAPACITY, io::stdout())) }
    }

    pub fn threaded() -> Self {
        let (tx, rx) = mpsc::channel::<Msg>();
        let handle = std::thread::Builder::new()
            .name("xezim-stdout".to_string())
            .spawn(move || {
                // Do NOT hold an exclusive `stdout.lock()` on the worker —
                // it would deadlock anything on the main thread that
                // touches stdout (e.g. `println!` after sim.run() returns).
                // The unlocked `Stdout` handle acquires the lock per write
                // and releases it between calls.
                let w = BufWriter::with_capacity(BUF_CAPACITY, io::stdout());
                run_threaded_writer(rx, w);
            })
            .expect("spawn xezim-stdout writer thread");
        StdoutSink {
            mode: Mode::Threaded {
                buf: Vec::with_capacity(BUF_CAPACITY),
                tx: Some(tx),
                handle: Some(handle),
            },
        }
    }

    #[inline]
    pub fn write_str(&mut self, s: &str) {
        self.write_bytes(s.as_bytes());
    }

    #[inline]
    pub fn writeln_str(&mut self, s: &str) {
        self.write_bytes(s.as_bytes());
        self.write_bytes(b"\n");
        // Line-buffered: flush after each complete line so $display output
        // is visible as the sim produces it. Without this, a long-running
        // sim (e.g. c910 hello_world) can churn for 30+ minutes with no
        // visible output because BufWriter's 16 KB capacity never fills
        // before a SIGTERM from `timeout`. `$write`-without-newline still
        // batches in the buffer (picorv32's per-char UART pattern isn't
        // regressed — `\n` from the surrounding message triggers flush).
        self.flush();
    }

    fn write_bytes(&mut self, data: &[u8]) {
        match &mut self.mode {
            Mode::Inline(w) => { let _ = w.write_all(data); }
            Mode::Threaded { buf, tx: Some(tx), .. } => {
                buf.extend_from_slice(data);
                if buf.len() >= FLUSH_THRESHOLD {
                    let chunk = std::mem::replace(buf, Vec::with_capacity(BUF_CAPACITY));
                    let _ = tx.send(Msg::Chunk(chunk));
                }
            }
            _ => {}
        }
    }

    pub fn flush(&mut self) {
        match &mut self.mode {
            Mode::Inline(w) => { let _ = w.flush(); }
            Mode::Threaded { buf, tx: Some(tx), .. } => {
                if !buf.is_empty() {
                    let chunk = std::mem::take(buf);
                    let _ = tx.send(Msg::FlushChunk(chunk));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct RecordingWriter(Arc<Mutex<Vec<&'static str>>>);

    impl Write for RecordingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().push("write");
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.lock().unwrap().push("flush");
            Ok(())
        }
    }

    #[test]
    fn threaded_chunks_are_visible_before_shutdown() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let writer = RecordingWriter(Arc::clone(&events));
        let (tx, rx) = mpsc::channel();
        tx.send(Msg::Chunk(b"buffered ".to_vec())).unwrap();
        tx.send(Msg::FlushChunk(b"line\n".to_vec())).unwrap();
        tx.send(Msg::Shutdown).unwrap();

        run_threaded_writer(rx, writer);

        assert_eq!(
            *events.lock().unwrap(),
            ["write", "write", "flush", "flush"]
        );
    }
}

impl Drop for StdoutSink {
    fn drop(&mut self) {
        self.flush();
        if let Mode::Threaded { tx, handle, .. } = &mut self.mode {
            if let Some(tx) = tx.take() {
                let _ = tx.send(Msg::Shutdown);
                drop(tx);
            }
            if let Some(h) = handle.take() {
                let _ = h.join();
            }
        }
    }
}
