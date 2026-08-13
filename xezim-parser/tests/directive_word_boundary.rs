//! IEEE 1800-2017 §22.5.1: a user macro name may merely START with a
//! compiler-directive keyword (`include_default_error_task`, `undefined_x`,
//! `ifdef_guard_y`). The line-based resolver matched directives by PREFIX,
//! so invoking such a macro was swallowed as a (malformed) directive —
//! `include_...` failed preprocessing outright with "malformed `include".

use sv_parser::preprocess;

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn macro_starting_with_include_expands() {
    let out = preprocess(
        "`define include_default_error_task int x = 5;\n`include_default_error_task\n",
    );
    assert!(
        norm(&out).contains("int x = 5"),
        "macro named include_* must expand, not parse as `include; got: {:?}",
        norm(&out)
    );
}

#[test]
fn macro_with_embedded_ifdef_body_expands_empty_when_undefined() {
    // The reported shape: a macro whose BODY is an `ifdef-guarded block,
    // invoked with the guard undefined — must expand to nothing.
    let out = preprocess(concat!(
        "`undef _COSIM_\n",
        "`define guarded_task \\\n",
        "`ifdef _COSIM_ \\\n",
        "   function void f(); endfunction \\\n",
        "`endif\n",
        "module m;\n",
        "`guarded_task\n",
        "endmodule\n",
    ));
    let n = norm(&out);
    assert!(
        !n.contains("function"),
        "guard undefined: body must vanish; got: {:?}",
        n
    );
    assert!(n.contains("module m"), "module survives; got: {:?}", n);
}

#[test]
fn directive_prefixed_macro_names_all_expand() {
    let out = preprocess(concat!(
        "`define undefined_marker 7\n",
        "`define ifdef_guard_val 3\n",
        "`define elsewhere 11\n",
        "`define endif_tag 13\n",
        "`define definitely 17\n",
        "`define includes_all 19\n",
        "int a = `undefined_marker + `ifdef_guard_val + `elsewhere + `endif_tag + `definitely + `includes_all;\n",
    ));
    let n = norm(&out);
    assert!(
        n.contains("int a = 7 + 3 + 11 + 13 + 17 + 19 ;")
            || n.contains("int a = 7 + 3 + 11 + 13 + 17 + 19;"),
        "all directive-prefixed macro names must expand; got: {:?}",
        n
    );
}
