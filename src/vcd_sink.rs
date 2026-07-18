//! Output sink for VCD / XTrace dumps.
//!
//! Two modes:
//!   * Inline   — writes straight to a `BufWriter<File>` on the caller thread.
//!   * Threaded — hands work to a dedicated writer thread. Two message
//!                kinds are carried:
//!                  - `Chunk(Vec<u8>)`: pre-formatted bytes (used for VCD
//!                    headers and anything written via
//!                    `std::io::Write`).
//!                  - `VcdBatch(Vec<VcdTimestep>)`: structured per-timestep
//!                    value changes. The worker thread formats them with
//!                    `write_vcd_value`. This moves the bit-by-bit ASCII
//!                    conversion off the main simulation thread, which is
//!                    the actual CPU bottleneck for VCD dumps.
//!                Batches are flushed when `pending.len() >=
//!                `VCD_BATCH_FLUSH` or at `commit()` / `Drop`.
//!
//! `VcdSink` implements `std::io::Write` so existing `writeln!(w, ...)` call
//! sites keep working unchanged.
//!
//! The underlying byte stream is a boxed `Write` ([`DumpWriter`]). For plain
//! dumps that is the file itself; for `.zst` dumps it is a streaming zstd
//! encoder (`auto_finish`, so the frame footer is written on drop — whether
//! that drop happens on the caller thread (inline) or the writer thread).

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

use super::value::{LogicBit, Value};

/// Owned byte sink behind a `VcdSink`. `Send` so the threaded writer can own it.
pub type DumpWriter = Box<dyn Write + Send>;

const CHUNK_CAPACITY: usize = 64 * 1024;
/// Minimum buffered bytes before `commit()` hands a byte chunk to the worker.
const COMMIT_THRESHOLD: usize = 32 * 1024;
/// Number of per-timestep VCD change records to accumulate before dispatch.
const VCD_BATCH_FLUSH: usize = 256;

pub struct VcdTimestep {
    /// `Some(t)` → emit `#t` header before the changes.
    pub time: Option<u64>,
    /// (VCD identifier code, value). The code is an `Arc<str>` so the caller's
    /// per-change clone (millions of times on large dumps) is a refcount bump
    /// instead of a fresh heap allocation of the short code string.
    pub changes: Vec<(Arc<str>, Value)>,
}

enum WorkerMsg {
    Chunk(Vec<u8>),
    VcdBatch(Vec<VcdTimestep>),
    /// Force the worker's `BufWriter` (and any streaming zstd encoder) to
    /// flush accumulated bytes to the OS file, so a later crash/SIGKILL of
    /// the main process leaves a readable partial dump.
    Flush,
    Shutdown,
}

enum Mode {
    Inline(BufWriter<DumpWriter>),
    Threaded {
        buf: Vec<u8>,
        pending: Vec<VcdTimestep>,
        tx: Option<Sender<WorkerMsg>>,
        handle: Option<JoinHandle<()>>,
    },
}

pub struct VcdSink {
    mode: Mode,
}

impl VcdSink {
    pub fn inline(w: DumpWriter) -> Self {
        VcdSink { mode: Mode::Inline(BufWriter::new(w)) }
    }

    pub fn threaded(w: DumpWriter) -> Self {
        let (tx, rx) = mpsc::channel::<WorkerMsg>();
        let handle = std::thread::Builder::new()
            .name("xezim-vcd".to_string())
            .spawn(move || {
                let mut bw = BufWriter::with_capacity(256 * 1024, w);
                while let Ok(msg) = rx.recv() {
                    match msg {
                        WorkerMsg::Chunk(bytes) => { let _ = bw.write_all(&bytes); }
                        WorkerMsg::VcdBatch(batch) => {
                            for ts in &batch {
                                if let Some(t) = ts.time {
                                    let _ = writeln!(bw, "#{}", t);
                                }
                                for (id, val) in &ts.changes {
                                    write_vcd_value(&mut bw, val, id);
                                }
                            }
                        }
                        WorkerMsg::Flush => { let _ = bw.flush(); }
                        WorkerMsg::Shutdown => break,
                    }
                }
                let _ = bw.flush();
                // `bw` drops here → flushes, then drops the inner `DumpWriter`.
                // For a zstd `auto_finish` encoder that drop writes the frame footer.
            })
            .expect("spawn xezim-vcd writer thread");
        VcdSink {
            mode: Mode::Threaded {
                buf: Vec::with_capacity(CHUNK_CAPACITY),
                pending: Vec::with_capacity(VCD_BATCH_FLUSH),
                tx: Some(tx),
                handle: Some(handle),
            },
        }
    }

    /// Open a dump sink writing to `file`.
    ///
    /// * `threaded` — route formatting/IO through a background writer thread.
    /// * `zstd_level` — `Some(level)` to zstd-compress the byte stream (the
    ///   produced file is a single `.zst` frame); `None` for a plain stream.
    pub fn open_file(file: File, threaded: bool, zstd_level: Option<i32>) -> io::Result<Self> {
        let w: DumpWriter = match zstd_level {
            Some(level) => Box::new(zstd::stream::Encoder::new(file, level)?.auto_finish()),
            None => Box::new(file),
        };
        Ok(if threaded { Self::threaded(w) } else { Self::inline(w) })
    }

    /// In threaded mode: push a timestep's value changes into the pending
    /// batch (dispatched when the batch is full). In inline mode: format
    /// immediately on the caller thread.
    pub fn post_vcd_changes(&mut self, time: Option<u64>, changes: Vec<(Arc<str>, Value)>) {
        match &mut self.mode {
            Mode::Inline(w) => {
                if let Some(t) = time {
                    let _ = writeln!(w, "#{}", t);
                }
                for (id, val) in &changes {
                    write_vcd_value(w, val, id);
                }
            }
            Mode::Threaded { buf, pending, tx: Some(tx), .. } => {
                if !buf.is_empty() {
                    let chunk = std::mem::replace(buf, Vec::with_capacity(CHUNK_CAPACITY));
                    let _ = tx.send(WorkerMsg::Chunk(chunk));
                }
                pending.push(VcdTimestep { time, changes });
                if pending.len() >= VCD_BATCH_FLUSH {
                    let batch = std::mem::replace(pending, Vec::with_capacity(VCD_BATCH_FLUSH));
                    let _ = tx.send(WorkerMsg::VcdBatch(batch));
                }
            }
            _ => {}
        }
    }

    /// Hand any pending bytes and VCD batches to the worker. In inline
    /// mode this is a no-op; `BufWriter` handles batching. Called at
    /// natural boundaries; `Drop` flushes whatever is left.
    pub fn commit(&mut self) {
        if let Mode::Threaded { buf, pending, tx: Some(tx), .. } = &mut self.mode {
            if buf.len() >= COMMIT_THRESHOLD {
                let chunk = std::mem::replace(buf, Vec::with_capacity(CHUNK_CAPACITY));
                let _ = tx.send(WorkerMsg::Chunk(chunk));
            }
            if pending.len() >= VCD_BATCH_FLUSH {
                let batch = std::mem::replace(pending, Vec::with_capacity(VCD_BATCH_FLUSH));
                let _ = tx.send(WorkerMsg::VcdBatch(batch));
            }
        }
    }
}

impl Write for VcdSink {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        match &mut self.mode {
            Mode::Inline(w) => w.write(data),
            Mode::Threaded { buf, pending, tx: Some(tx), .. } => {
                if !pending.is_empty() {
                    let batch = std::mem::replace(pending, Vec::with_capacity(VCD_BATCH_FLUSH));
                    let _ = tx.send(WorkerMsg::VcdBatch(batch));
                }
                buf.extend_from_slice(data);
                Ok(data.len())
            }
            Mode::Threaded { buf, .. } => {
                buf.extend_from_slice(data);
                Ok(data.len())
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.mode {
            Mode::Inline(w) => w.flush(),
            // Unlike `commit()` (threshold-gated), a flush must force ALL
            // buffered work to the worker AND have the worker flush its own
            // BufWriter to disk — otherwise a crash loses the tail of the dump.
            Mode::Threaded { buf, pending, tx: Some(tx), .. } => {
                if !buf.is_empty() {
                    let chunk = std::mem::replace(buf, Vec::with_capacity(CHUNK_CAPACITY));
                    let _ = tx.send(WorkerMsg::Chunk(chunk));
                }
                if !pending.is_empty() {
                    let batch = std::mem::replace(pending, Vec::with_capacity(VCD_BATCH_FLUSH));
                    let _ = tx.send(WorkerMsg::VcdBatch(batch));
                }
                let _ = tx.send(WorkerMsg::Flush);
                Ok(())
            }
            Mode::Threaded { .. } => Ok(()),
        }
    }
}

impl Drop for VcdSink {
    fn drop(&mut self) {
        if let Mode::Threaded { buf, pending, tx, handle, .. } = &mut self.mode {
            if let Some(tx_ref) = tx.as_ref() {
                if !buf.is_empty() {
                    let chunk = std::mem::take(buf);
                    let _ = tx_ref.send(WorkerMsg::Chunk(chunk));
                }
                if !pending.is_empty() {
                    let batch = std::mem::take(pending);
                    let _ = tx_ref.send(WorkerMsg::VcdBatch(batch));
                }
            }
            if let Some(tx) = tx.take() {
                let _ = tx.send(WorkerMsg::Shutdown);
                drop(tx);
            }
            if let Some(h) = handle.take() {
                let _ = h.join();
            }
        }
    }
}

/// Render a `real` value as a VCD decimal number (IEEE 1800-2017 §21.7.2.1:
/// the value of a `real` variable is written as `r<decimal_number>`).
/// Rust's `{}` for `f64` is the shortest round-trip form, which is exactly
/// what a VCD reader needs; NaN/±inf have no VCD spelling, so they degrade
/// to `0` rather than emitting an unparsable token.
pub fn vcd_real_string(v: f64) -> String {
    if v.is_finite() {
        format!("{}", v)
    } else {
        "0".to_string()
    }
}

/// The binary digit string of a vector value, MSB first, with §21.7.2.1-legal
/// leading-run suppression — the same spelling a reference simulator Verilog emits.
///
/// A reader LEFT-EXTENDS a value shorter than the `$var` width using the
/// leftmost emitted character: `x` extends with x, `z` with z, and anything else
/// (`0`/`1`) with `0`. So:
///
///   * a leading run of `x` collapses to ONE `x` (`8'bxxxx0011` → `bx0011`), and
///     likewise a leading run of `z` (`8'bzzzz0011` → `bz0011`) — the reader
///     re-extends with that same character.
///   * a leading run of `0` collapses only while the first RETAINED character is
///     `1` (`8'b00001111` → `b1111`). `8'b000000x1` may NOT collapse to `bx1` —
///     that reads back as `8'bxxxxxxx1` — so one explicit `0` is kept: `b0x1`.
///   * a leading `1` extends with `0`, so nothing may be dropped in front of it.
pub fn vcd_vector_bits(val: &Value) -> String {
    let w = val.width as usize;
    let mut s = String::with_capacity(w + 1);
    for i in (0..w).rev() {
        s.push(match val.get_bit(i) {
            LogicBit::Zero => '0',
            LogicBit::One => '1',
            LogicBit::X => 'x',
            LogicBit::Z => 'z',
        });
    }
    let lead = match s.as_bytes().first() {
        Some(&c) => c,
        None => return "0".to_string(),
    };
    // `1` left-extends as `0`: the leading run is significant, keep it all.
    if lead == b'1' {
        return s;
    }
    // Index of the first character that differs from the leading one.
    let end = match s.bytes().position(|c| c != lead) {
        // Uniform vector: one character stands for all of it (`bx`, `bz`, `b0`).
        None => return (lead as char).to_string(),
        Some(i) => i,
    };
    if lead == b'0' {
        if s.as_bytes()[end] == b'1' {
            // 0-extension restores the dropped zeros.
            return s.split_off(end);
        }
        // First significant bit is x/z: keep ONE `0` so the reader 0-extends
        // instead of x/z-extending.
        let mut out = String::with_capacity(w - end + 1);
        out.push('0');
        out.push_str(&s[end..]);
        return out;
    }
    // Leading run of x (or z): the reader re-extends with that same character,
    // so one instance carries the whole run.
    let mut out = String::with_capacity(w - end + 1);
    out.push(lead as char);
    out.push_str(&s[end..]);
    out
}

/// Format a single `Value` as a VCD value-change record (real, scalar or
/// vector) — IEEE 1800-2017 §21.7.2.1. Shared by the inline path, the
/// background writer thread AND `Simulator`'s header/checkpoint paths, which
/// used to carry a second, divergent copy of this logic.
pub fn write_vcd_value<W: Write>(w: &mut W, val: &Value, id: &str) {
    if val.is_real {
        // `real` is a `$var real 64` and its changes are `r<decimal> <id>`.
        // Emitting the raw IEEE-754 bit pattern as a 64-bit binary vector
        // (the old behaviour) makes every real read back as a nonsense integer.
        let _ = writeln!(w, "r{} {}", vcd_real_string(val.to_f64()), id);
    } else if val.width == 1 {
        let ch = match val.bits_first() {
            LogicBit::Zero => '0',
            LogicBit::One => '1',
            LogicBit::X => 'x',
            LogicBit::Z => 'z',
        };
        let _ = writeln!(w, "{}{}", ch, id);
    } else {
        let _ = writeln!(w, "b{} {}", vcd_vector_bits(val), id);
    }
}
