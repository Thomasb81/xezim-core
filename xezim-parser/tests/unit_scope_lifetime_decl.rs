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
    parse(src)
        .errors
        .iter()
        .map(|e| format!("{:?}", e))
        .collect()
}

#[test]
fn unit_scope_static_declaration_parses() {
    let e = errors("static int counter = 0;\nmodule m; endmodule\n");
    assert!(
        e.is_empty(),
        "`static` at $unit scope must parse, got: {:?}",
        e
    );
}

#[test]
fn unit_scope_automatic_declaration_parses() {
    let e = errors("automatic int counter = 0;\nmodule m; endmodule\n");
    assert!(
        e.is_empty(),
        "`automatic` at $unit scope must parse, got: {:?}",
        e
    );
}

#[test]
fn unit_scope_static_vector_declaration_parses() {
    let e = errors("static logic [7:0] flags = 0;\nmodule m; endmodule\n");
    assert!(
        e.is_empty(),
        "a lifetime on a vector decl must parse, got: {:?}",
        e
    );
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
        assert!(
            e.is_empty(),
            "`{}` must still parse, got: {:?}",
            src.lines().next().unwrap(),
            e
        );
    }
}

/// §6.7 net declarations at compilation-unit scope (`wire x;`). These are
/// legal at $unit (like `int a;`), but the $unit dispatch guard only admitted
/// data-type keywords, so a leading net type fell through to an "unexpected
/// token". The issue's `wire x;` / `wire x2[2], y2[2];` died that way.
#[test]
fn unit_scope_net_declaration_parses() {
    for src in [
        "wire x;\nmodule m; endmodule\n",
        "wire x2[2], y2[2];\nmodule m; endmodule\n",
        "tri1 a, b;\nmodule m; endmodule\n",
        "wire [3:0] w;\nmodule m; endmodule\n",
        "wand b1, b2;\nmodule m; endmodule\n",
    ] {
        let e = errors(src);
        assert!(
            e.is_empty(),
            "`{}` at $unit scope must parse, got: {:?}",
            src.lines().next().unwrap(),
            e
        );
    }
}

/// A `$unit` net next to a variable is fine and both survive to elaboration
/// (the net is hoisted like a $unit variable so module references resolve).
#[test]
fn unit_scope_net_variable_mix_parses() {
    let e = errors("wire x2[2], y2[2];\nint a;\nmodule m; endmodule\n");
    assert!(e.is_empty(), "net+var at $unit must parse, got: {:?}", e);
}
