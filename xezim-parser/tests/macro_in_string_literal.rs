//! IEEE 1800.1-2023 §22.5.1 "Macro definition and usage": macro expansion
//! does **not** occur inside string literals.
//!
//! `expand_macros_once` walked the token stream with no string-literal
//! tracking, so a backtick-prefixed *defined* macro name that appeared inside
//! a `"..."` literal was expanded as if it were real code. When that macro was
//! parameterized and the surrounding string text was not a `(`, strict mode
//! falsely rejected the whole translation unit:
//!
//! ```text
//! `define uvm_object_utils(T) ...
//! $display("get_type function implemented by `uvm_object_utils ...");
//!   -> "macro `uvm_object_utils` requires parentheses (§22.5.1)"
//! ```
//!
//! The fix tracks `"..."` state in the expansion loop (honoring `\"` escapes),
//! copying bytes verbatim while inside a string. These tests pin both facets:
//! the macro name is left untouched inside a string, and a macro is still
//! expanded OUTSIDE a string. The checks run at the preprocessor level
//! (string in / string out + the strict-mode error list).

use sv_parser::preprocessor::Preprocessor;

/// Preprocess and return (expanded_source, strict_errors).
fn run(src: &str) -> (String, Vec<String>) {
    let mut pp = Preprocessor::new();
    let out = pp.preprocess(src);
    (out, pp.errors().to_vec())
}

/// Collapse whitespace so assertions don't depend on how spacing lands.
fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A parameterized macro name inside a string literal is NOT expanded: the
/// literal text is preserved verbatim, and no "requires parentheses" error is
/// raised. The same macro OUTSIDE the string still expands.
#[test]
fn parameterized_macro_in_string_is_literal_and_outside_expands() {
    let (out, errs) = run(
        r#"
`define FOO(x) ((x)+1)
module top;
  int a;
  initial begin
    a = `FOO(5);
    $display("lit `FOO stays literal end");
  end
endmodule
"#,
    );
    let n = norm(&out);
    // Outside the string: macro expanded to ((5)+1).
    assert!(
        n.contains("a = ((5)+1)"),
        "macro outside string should expand; got: {:?}",
        n
    );
    // Inside the string: macro name preserved verbatim.
    assert!(
        n.contains("`FOO stays literal end"),
        "macro name inside string must be literal; got: {:?}",
        n
    );
    // No spurious strict-mode error.
    assert!(
        !errs.iter().any(|e| e.contains("requires parentheses")),
        "expected no 'requires parentheses' error; got: {:?}",
        errs
    );
}

/// An object-like (no-arg) macro name inside a string is also left untouched.
#[test]
fn object_macro_in_string_is_literal() {
    let (out, errs) = run(
        r#"
`define SIZE 8
module top;
  int w;
  initial begin
    w = `SIZE;
    $display("width is `SIZE bits");
  end
endmodule
"#,
    );
    let n = norm(&out);
    assert!(n.contains("w = 8"), "expansion outside string; got: {:?}", n);
    assert!(
        n.contains("`SIZE bits"),
        "object macro name inside string must be literal; got: {:?}",
        n
    );
    assert!(errs.is_empty(), "expected no errors; got: {:?}", errs);
}

/// A macro name followed by non-'(' text inside a string is the exact case
/// that pre-fix raised a false "requires parentheses" error (because the
/// expanded macro had no parens). Post-fix it is literal text and no error.
#[test]
fn parameterized_macro_in_string_emits_no_paren_error() {
    let (out, errs) = run(
        r#"
`define BAR(y) (2*(y))
module top;
  initial begin
    $display("see `BAR here");
    $display("ok=%0d", `BAR(3));
  end
endmodule
"#,
    );
    let n = norm(&out);
    assert!(
        n.contains("see `BAR here"),
        "parameterized macro name in string must be literal; got: {:?}",
        n
    );
    assert!(
        !errs.iter().any(|e| e.contains("requires parentheses")),
        "expected no 'requires parentheses' error for a macro name in a string; got: {:?}",
        errs
    );
}

/// An escaped quote inside a string (`\"`) does not prematurely end the
/// string: the macro name after the escaped quote is still literal.
#[test]
fn escaped_quote_does_not_end_the_string() {
    let (out, errs) = run(
        r#"
`define QX(x) (x)
module top;
  initial begin
    $display("he said \"hi\" then `QX end");
  end
endmodule
"#,
    );
    let n = norm(&out);
    assert!(
        n.contains("`QX end"),
        "macro name after an escaped quote must still be literal; got: {:?}",
        n
    );
    assert!(
        !errs.iter().any(|e| e.contains("requires parentheses")),
        "expected no error; got: {:?}",
        errs
    );
}
