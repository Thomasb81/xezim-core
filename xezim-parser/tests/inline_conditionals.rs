//! IEEE 1800-2017 §22.6: `` `ifdef ``/`` `ifndef ``/`` `elsif ``/`` `else ``/
//! `` `endif `` may appear MID-LINE, not only at the start of a line.
//!
//! UVM 2020.3.1 writes
//!   `static `ifndef UVM_ENABLE_DEPRECATED_API local `endif bit m;`
//! The line-based directive resolver only recognised a directive at the start
//! of a line, so the inline form passed through verbatim and the parser choked
//! on the `local` keyword — the whole UVM 2020 package failed to compile.

use sv_parser::preprocess;

/// Collapse whitespace so the assertions don't depend on how the split lands.
fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn inline_ifndef_keeps_the_body_when_the_macro_is_undefined() {
    let out = preprocess("static `ifndef FOO local `endif bit x;\n");
    assert!(
        norm(&out).contains("static local bit x ;")
            || norm(&out).contains("static local bit x;"),
        "inline `ifndef should keep `local` when FOO is undefined; got: {:?}",
        norm(&out)
    );
}

#[test]
fn inline_ifndef_drops_the_body_when_the_macro_is_defined() {
    let out = preprocess("`define FOO\nstatic `ifndef FOO local `endif bit x;\n");
    let n = norm(&out);
    assert!(n.contains("static bit x"), "expected `static bit x`, got: {:?}", n);
    assert!(!n.contains("local"), "`local` must be dropped when FOO is defined; got: {:?}", n);
}

#[test]
fn inline_ifdef_keeps_the_body_when_the_macro_is_defined() {
    let out = preprocess("`define FOO\nstatic `ifdef FOO local `endif bit x;\n");
    assert!(norm(&out).contains("static local bit x"), "got: {:?}", norm(&out));
}

#[test]
fn inline_conditional_inside_a_string_is_not_treated_as_a_directive() {
    // A backtick inside a string literal is data, not a directive.
    let out = preprocess("string s = \"a `ifndef b\";\nint y;\n");
    assert!(norm(&out).contains("`ifndef"), "string content must be preserved: {:?}", norm(&out));
}

#[test]
fn a_conditional_in_a_define_body_is_not_split() {
    // The `ifdef inside a macro BODY belongs to the body; splitting it at
    // define time would break the `define. The pre-pass must leave the define
    // line untouched, so code AFTER it preprocesses cleanly. (Whether xezim
    // then resolves a body conditional on expansion is a separate matter and
    // not what this fix touches.)
    let out = preprocess("`define PICK `ifdef FOO 1 `else 2 `endif\nint after = 7;\n");
    assert!(
        norm(&out).contains("int after = 7"),
        "a `define with an inline conditional in its body must not disturb later code: {:?}",
        norm(&out)
    );
}
