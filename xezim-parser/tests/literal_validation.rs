//! Regression tests for malformed numeric / string literal diagnostics
//! (IEEE 1800-2017 §5.7 literals, §5.9 strings).
//!
//! Each of these malformed literals previously reached the value layer and was
//! silently coerced to a wrong value (a real `0`, an all-X vector, a 0-width
//! constant, or a mangled string). They must now produce a diagnostic while the
//! well-formed forms continue to parse cleanly.

use sv_parser::parse;

/// Parse `expr` as the RHS of a continuous assign and return whether the parse
/// produced any error diagnostic.
fn has_error(expr: &str) -> bool {
    let src = format!("module t; real r; logic [31:0] a; initial begin r = 0.0; a = {}; end endmodule", expr);
    !parse(&src).errors.is_empty()
}

fn stmt_has_error(stmt: &str) -> bool {
    let src = format!("module t; real r; logic [31:0] a; initial begin {} end endmodule", stmt);
    !parse(&src).errors.is_empty()
}

// ---------------------------------------------------------------------------
// L1 — real literal must have exponent digits (§5.7.2)
// ---------------------------------------------------------------------------

#[test]
fn l1_real_missing_exponent_digits_errors() {
    assert!(stmt_has_error("r = 1.0e+;"), "1.0e+ must error, not become 0");
    assert!(stmt_has_error("r = 1.0e;"), "1.0e must error");
    assert!(stmt_has_error("r = 1.0e-;"), "1.0e- must error");
}

#[test]
fn l1_valid_reals_still_parse() {
    assert!(!has_error("1.0e3"), "1.0e3 must parse");
    assert!(!has_error("2.5e-2"), "2.5e-2 must parse");
    assert!(!has_error("1.0e10"), "1.0e10 must parse");
    assert!(!has_error("1.0e+3"), "1.0e+3 must parse");
    assert!(!has_error("3.14159"), "plain real must parse");
}

// ---------------------------------------------------------------------------
// L2 — decimal based literal: at most a single x or z (§5.7.1)
// ---------------------------------------------------------------------------

#[test]
fn l2_decimal_multi_or_mixed_xz_errors() {
    assert!(has_error("8'dxx"), "8'dxx must error");
    assert!(has_error("8'd1x"), "8'd1x must error");
    assert!(has_error("8'dzz"), "8'dzz must error");
    assert!(has_error("8'dxz"), "8'dxz (mixed) must error");
}

#[test]
fn l2_decimal_single_xz_ok() {
    assert!(!has_error("8'dx"), "8'dx must parse");
    assert!(!has_error("8'dz"), "8'dz must parse");
    assert!(!has_error("8'd?"), "8'd? must parse");
    assert!(!has_error("8'd5"), "8'd5 must parse");
    // Higher radices may legitimately carry multiple x/z digits.
    assert!(!has_error("8'hxx"), "8'hxx must parse");
    assert!(!has_error("8'bzz"), "8'bzz must parse");
}

// ---------------------------------------------------------------------------
// L3 — based literal size must be > 0 (§5.7.1)
// ---------------------------------------------------------------------------

#[test]
fn l3_zero_size_errors() {
    assert!(has_error("0'd5"), "0'd5 must error");
    assert!(has_error("0'h0"), "0'h0 must error");
    assert!(has_error("0'b0"), "0'b0 must error");
}

#[test]
fn l3_valid_sizes_ok() {
    assert!(!has_error("1'd1"), "1'd1 must parse");
    assert!(!has_error("8'd5"), "8'd5 must parse");
    assert!(!has_error("32'hDEAD_BEEF"), "32'hDEAD_BEEF must parse");
    assert!(!has_error("8'hFF"), "8'hFF must parse");
}

// ---------------------------------------------------------------------------
// Unsized/unbased fills must remain valid (§5.7.1)
// ---------------------------------------------------------------------------

#[test]
fn unbased_unsized_fills_ok() {
    for lit in ["'0", "'1", "'x", "'z"] {
        assert!(!has_error(lit), "{} must parse", lit);
    }
}

// ---------------------------------------------------------------------------
// L4 — string escape validation (§5.9)
// ---------------------------------------------------------------------------

fn string_errors(lit: &str) -> bool {
    // lit includes the surrounding quotes
    let src = format!("module t; initial $display({}); endmodule", lit);
    !parse(&src).errors.is_empty()
}

fn string_warns(lit: &str) -> bool {
    let src = format!("module t; initial $display({}); endmodule", lit);
    !parse(&src).warnings.is_empty()
}

#[test]
fn l4_bad_hex_escape_errors() {
    assert!(string_errors("\"a\\xGGb\""), "\\x with no hex digit must error");
    assert!(string_errors("\"\\x\""), "\\x at end of string must error");
}

#[test]
fn l4_valid_escapes_ok() {
    assert!(!string_errors("\"\\x41\""), "\\x41 must parse");
    assert!(!string_errors("\"\\101\""), "\\101 (octal) must parse");
    assert!(!string_errors("\"a\\nb\""), "\\n must parse");
    assert!(!string_errors("\"a\\tb\\\\c\\\"d\""), "\\t \\\\ \\\" must parse");
}

#[test]
fn l4_unknown_escape_warns_not_errors() {
    assert!(!string_errors("\"a\\qb\""), "unknown escape must not be a hard error");
    assert!(string_warns("\"a\\qb\""), "unknown escape must warn (not silently mangle)");
}
