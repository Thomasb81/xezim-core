//! Line map from preprocessed text back to the original sources.
//!
//! The preprocessor splices `include`d files in whole, expands macros (whose
//! bodies may span lines) and splits inline conditional directives onto
//! lines of their own, so a line of preprocessed text can come from another
//! file, from a macro body, or from the middle of an original line. Every
//! span the parser produces indexes the preprocessed text; this map is what
//! turns one back into `file:line:col` for a diagnostic.

use crate::ast::Span;
use crate::diagnostics::Location;

/// A file (or `line-directive name) that contributed text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapFile {
    pub path: String,
    /// `(file index, line)` of the `include that pulled this file in.
    pub included_from: Option<(u32, u32)>,
}

/// Where one line of preprocessed output came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineOrigin {
    pub file: u32,
    /// 1-based line in `file`.
    pub line: u32,
    /// 0-based character column in the original line where this output
    /// line starts (non-zero only for a line split off an original line).
    pub col: u32,
    /// 1 + index into `LineMap::macros` for macro-expanded text, else 0.
    pub mac: u32,
    /// Macro-expanded text: column of the invocation, relative to `col`.
    pub mcol: u32,
    /// Macro-expanded text: the output line still starts with the original
    /// text before the invocation (the first line of an expansion).
    pub mfirst: bool,
    /// Macro-expanded text: column (relative to `col`) where the FIRST
    /// line's substituted text ends, exclusive. Ordinary tokens that merely
    /// FOLLOW a macro call on the same line sit at `>= mcol + mend` and are
    /// original text, not macro text. On `mfirst == false` (continuation
    /// lines of a multi-line expansion) every token is macro text and this
    /// value is ignored. A value of 0 means "extent unknown": the caller
    /// falls back to treating the whole tail of the line as macro text
    /// (the historical behaviour).
    pub mend: u32,
    /// Macro-expanded text: character length of the FIRST macro invocation
    /// (`` `NAME `` plus any balanced `(...)`) in the ORIGINAL line. When a
    /// token after the substitution keeps its own column, the substitution
    /// may change length (`mcall` input chars became `mend` output chars), so
    /// the OFFSET of the following original text is shifted by `mcall - mend`.
    /// Zero means unknown (no adjustment).
    pub mcall: u32,
}

impl LineOrigin {
    pub(crate) fn plain(file: u32, line: u32, col: u32) -> Self {
        LineOrigin {
            file,
            line,
            col,
            mac: 0,
            mcol: 0,
            mfirst: false,
            mend: 0,
            mcall: 0,
        }
    }
}

/// A run of output lines whose origins advance by `step` lines each (`None`
/// until a second line joins the run).
#[derive(Debug, Clone, Copy)]
struct Seg {
    out: u32,
    origin: LineOrigin,
    step: Option<u32>,
}

/// Maps each line of one preprocessed text to its origin.
#[derive(Debug, Clone, Default)]
pub struct LineMap {
    pub files: Vec<MapFile>,
    macros: Vec<String>,
    segs: Vec<Seg>,
}

impl LineMap {
    pub(crate) fn new(files: Vec<MapFile>, macros: Vec<String>, origins: &[LineOrigin]) -> Self {
        let mut segs: Vec<Seg> = Vec::new();
        for (i, o) in origins.iter().enumerate() {
            let i = i as u32;
            if let Some(last) = segs.last_mut() {
                let n = i - last.out;
                let o_at = LineOrigin {
                    line: last.origin.line,
                    ..*o
                };
                if o_at == last.origin {
                    match last.step {
                        None if n == 1 && o.line == last.origin.line + 1 => {
                            last.step = Some(1);
                            continue;
                        }
                        None if n == 1 && o.line == last.origin.line => {
                            last.step = Some(0);
                            continue;
                        }
                        Some(step) if o.line == last.origin.line + n * step => continue,
                        _ => {}
                    }
                }
            }
            segs.push(Seg {
                out: i,
                origin: *o,
                step: None,
            });
        }
        LineMap {
            files,
            macros,
            segs,
        }
    }

    /// Origin of output line `out` (0-based).
    pub(crate) fn origin(&self, out: usize) -> Option<LineOrigin> {
        let out = u32::try_from(out).ok()?;
        let k = self.segs.partition_point(|s| s.out <= out).checked_sub(1)?;
        let s = &self.segs[k];
        let mut o = s.origin;
        o.line += (out - s.out) * s.step.unwrap_or(0);
        Some(o)
    }

    /// Original file and 1-based line of output line `out` (0-based); for
    /// macro-expanded text, the line of the invocation. Lets a caller that
    /// resolves many spans count output lines itself, once per text.
    pub fn file_line(&self, out: usize) -> Option<(&str, u32)> {
        let o = self.origin(out)?;
        Some((self.files.get(o.file as usize)?.path.as_str(), o.line))
    }

    /// Include chain of file `f`, innermost first.
    fn include_chain(&self, mut f: u32) -> Vec<(String, u32)> {
        let mut chain = Vec::new();
        while let Some((parent, line)) = self.files.get(f as usize).and_then(|m| m.included_from) {
            let Some(p) = self.files.get(parent as usize) else {
                break;
            };
            chain.push((p.path.clone(), line));
            if chain.len() > 64 {
                break;
            }
            f = parent;
        }
        chain
    }

    /// Resolve `span` (a byte range of the preprocessed `text` this map was
    /// built for) to the original source position.
    pub fn locate(&self, text: &str, span: Span) -> Option<Location> {
        let start = span.start.min(text.len());
        let out_line = text.as_bytes()[..start]
            .iter()
            .filter(|&&b| b == b'\n')
            .count();
        let o = self.origin(out_line)?;
        let file = self.files.get(o.file as usize)?;
        let line_start = text[..start].rfind('\n').map_or(0, |p| p + 1);
        let line_end = text[start..].find('\n').map_or(text.len(), |p| start + p);
        let out_col = text[line_start..start].chars().count() as u32;
        let end = span.end.clamp(start, line_end);
        let len = text[start..end].chars().count().max(1) as u32;
        let mut loc = Location {
            file: file.path.clone(),
            line: o.line,
            col: o.col + out_col + 1,
            len,
            macro_name: None,
            included_from: self.include_chain(o.file),
            fallback_text: None,
        };
        // Canonical macro-text predicate on the expansion's first line: a
        // token is macro text only while its column lies within the
        // substituted run [mcol, mcol+mend). Ordinary tokens before the call
        // (out_col < mcol) and -- crucially -- ordinary tokens that merely
        // FOLLOW the call on the same line (out_col >= mcol+mend) are
        // original text and keep their real column. That second bound was the
        // defect: the old code redirected every token at/after the invocation
        // to the macro, so `logic [`W-1:0] data;` misreported `data` (which
        // comes after the expansion) at the invocation column.
        let in_macro_text = o.mac != 0
            && (!o.mfirst
                || (out_col >= o.mcol && (o.mend == 0 || out_col < o.mcol + o.mend)));
        if in_macro_text {
            // Macro text: point at the invocation (text left of it on the
            // expansion's first line is still the original).
            let name = self
                .macros
                .get(o.mac as usize - 1)
                .cloned()
                .unwrap_or_default();
            loc.col = o.col + o.mcol + 1;
            loc.len = name.chars().count() as u32 + 1;
            loc.macro_name = Some(name);
        } else {
            // Ordinary text. If this is an expansion line and the token lies
            // AFTER the macro substitution (out_col >= mcol + mend), the
            // expansion may have changed the line's length: the substitution
            // replaced `mcall` input chars with `mend` output chars, so the
            // original offset is `out_col + (mcall as i64 - mend as i64)`.
            // Without this the token keeps its preprocessed column, which is
            // wrong when the expansion shrank/grew (`` `W `` -> `8` moves
            // `data` left by one). Only the FIRST expansion on the line is
            // modelled; exotic multi-macro lines degrade to the unadjusted
            // column, which is still far better than the old misattribute.
            let mid = o.mcol + o.mend;
            if o.mac != 0 && o.mfirst && o.mend != 0 && out_col >= mid && o.mcall != 0 {
                let shift = o.mcall as i64 - o.mend as i64;
                loc.col = (o.col as i64 + out_col as i64 + shift + 1).max(0) as u32;
            }
            if o.mac == 0 {
                loc.fallback_text = Some(text[line_start..line_end].to_string());
            }
        }
        Some(loc)
    }

    /// `file:line:col` of `span`, or `None` when the map does not cover it.
    pub fn location_string(&self, text: &str, span: Span) -> Option<String> {
        self.locate(text, span).map(|l| l.short())
    }
}
