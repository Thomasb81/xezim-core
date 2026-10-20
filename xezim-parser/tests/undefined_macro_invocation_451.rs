//! ISSUE #451: an undefined text-macro *invocation* (`\`NAME(a, b)`) is left as
//! literal text by preprocessing (§22.5.1/§22.5.1.1), so the parser used to see
//! the macro name followed by an argument list that isn't legal call syntax in
//! that position and reported every comma/paren as its own error — a wall of
//! derivative diagnostics, spilling onto following lines.
//!
//! The preprocessor now collapses the balanced `(...)` group of an undefined
//! macro *invocation*, leaving only the inert literal token — exactly like the
//! bare undefined-macro case — so the containing statement still parses and no
//! derivative errors accumulate.

use sv_parser::preprocess;

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn undefined_invocation_argument_list_is_collapsed() {
    // The `(1, 2)` argument list must be dropped entirely; only the literal
    // `\`UNDEFINED_MACRO` token survives, so a following statement is intact.
    let out = preprocess(
        "`define FOO 1\n\
         module m;\n\
         int x;\n\
         initial begin\n\
           `UNDEFINED_MACRO(1, 2)\n\
           x = 1;\n\
         end\n\
         endmodule\n",
    );
    let got = norm(&out);
    assert!(
        got.contains("`UNDEFINED_MACRO x = 1 ;")
            || got.contains("`UNDEFINED_MACRO x = 1;"),
        "argument list must be removed, following statement intact; got: {got:?}"
    );
    assert!(
        !got.contains("( 1 , 2 )"),
        "invocation args must be swallowed; got: {got:?}"
    );
}

#[test]
fn collapse_is_balanced_and_string_aware() {
    // Nested parens and a comma/paren inside a string literal must not break
    // the group; the whole thing is one inert token.
    let out = preprocess(
        "`define FOO 1\nmodule m;\ninitial `UNDEFINED_MACRO(f(a(b,2)), \"x)\", {1,2})\nendmodule\n",
    );
    let got = norm(&out);
    assert!(
        got.contains("`UNDEFINED_MACRO module")
            || got.contains("`UNDEFINED_MACRO endmodule"),
        "balanced group must collapse to the bare token; got: {got:?}"
    );
    assert!(
        !got.contains("f ( a ( b , 2 )")
            && !got.contains("\" x ) \"")
            && !got.contains("{ 1 , 2 }"),
        "unexpected argument text left in stream; got: {got:?}"
    );
}

#[test]
fn empty_argument_list_is_collapsed() {
    let out = preprocess("`define FOO 1\nmodule m;\ninitial `GONE()\nendmodule\n");
    let got = norm(&out);
    assert!(
        got.contains("`GONE") && !got.contains('('),
        "empty invocation must collapse to the bare token with no parens; got: {got:?}"
    );
}

#[test]
fn bare_undefined_macro_is_unchanged() {
    // No parens following: the literal token must pass through untouched.
    let out = preprocess("`define FOO 1\nmodule m;\n`GONE_MACRO x = 1;\nendmodule\n");
    let got = norm(&out);
    assert!(
        got.contains("`GONE_MACRO x = 1 ;") || got.contains("`GONE_MACRO x = 1;"),
        "bare undefined macro is left as literal text; got: {got:?}"
    );
}

#[test]
fn defined_macro_invocation_still_expands() {
    // Defined macros must expand normally — the collapse only applies to
    // UNDEFINED references, never to a `define`d one.
    let out = preprocess(
        "`define ADD(a, b) ((a) + (b))\n\
         module m;\n\
         int x = `ADD(1, 2);\n\
         endmodule\n",
    );
    let got = norm(&out);
    assert!(
        got.contains("int x = (( 1 ) + ( 2 )) ;")
            || got.contains("int x = ((1) + (2));")
            || got.contains("int x = (( 1 ) + ( 2 ));"),
        "defined macro must expand to its body; got: {got:?}"
    );
}

#[test]
fn collapse_handles_whitespace_before_paren() {
    let out = preprocess("`define FOO 1\nmodule m;\ninitial `GONE  (1, 2)\nendmodule\n");
    let got = norm(&out);
    assert!(
        got.contains("`GONE module") || got.contains("`GONE endmodule"),
        "whitespace before the invocation paren must still collapse; got: {got:?}"
    );
}