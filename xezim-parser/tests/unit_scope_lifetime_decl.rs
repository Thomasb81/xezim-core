//! §A.2.1.3 / §6.21: a `data_declaration` may carry an explicit LIFETIME
//! (`static` / `automatic`) before its type, at compilation-unit ($unit) scope
//! as well as anywhere else.
//!
//! `parse_data_declaration` has always consumed the lifetime, but the $unit
//! dispatch guard did not ADMIT one — it accepted a builtin type keyword,
//! `var`, `const`, or a user-defined type name, and nothing else. So the usual
//! way a testbench keeps a shared counter next to its macros,
//! `static int tests_failed = 0;`, died at the very first token with
//! "unexpected token: static" and took the whole file with it.

use sv_parser::parse;

fn errors(src: &str) -> Vec<String> {
    parse(src).errors.iter().map(|e| format!("{:?}", e)).collect()
}

#[test]
fn unit_scope_static_declaration_parses() {
    let e = errors("static int counter = 0;\nmodule m; endmodule\n");
    assert!(e.is_empty(), "`static` at $unit scope must parse, got: {:?}", e);
}

#[test]
fn unit_scope_automatic_declaration_parses() {
    let e = errors("automatic int counter = 0;\nmodule m; endmodule\n");
    assert!(e.is_empty(), "`automatic` at $unit scope must parse, got: {:?}", e);
}

#[test]
fn unit_scope_static_vector_declaration_parses() {
    let e = errors("static logic [7:0] flags = 0;\nmodule m; endmodule\n");
    assert!(e.is_empty(), "a lifetime on a vector decl must parse, got: {:?}", e);
}

/// The forms that already worked must keep working — the guard was widened,
/// not replaced.
#[test]
fn unit_scope_plain_forms_still_parse() {
    for src in [
        "int counter = 0;\nmodule m; endmodule\n",
        "const int limit = 4;\nmodule m; endmodule\n",
        "var int counter;\nmodule m; endmodule\n",
        "string label = \"x\";\nmodule m; endmodule\n",
    ] {
        let e = errors(src);
        assert!(e.is_empty(), "`{}` must still parse, got: {:?}", src.lines().next().unwrap(), e);
    }
}
