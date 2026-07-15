//! §16.12.9: the `not` property operator must parse inside a concurrent
//! assertion. Previously `assert property (@(posedge clk) not (...))` failed
//! with "expected expression, found KwNot 'not'" — the parser knew `not` only
//! as a gate primitive, not as a property operator.

use sv_parser::parse;

fn errors(src: &str) -> Vec<String> {
    parse(src).errors.iter().map(|e| format!("{:?}", e)).collect()
}

/// The reporter's shape: `not` wrapping a boolean property expression.
#[test]
fn not_property_operator_parses() {
    let src = r#"
module m(input logic clk, rst, logic [3:0] a, b);
  ap: assert property (
    @(posedge clk) not (~rst && ( $isunknown(|(a&b)) ))
  ) begin end else begin end
endmodule
"#;
    let e = errors(src);
    assert!(e.is_empty(), "`not` property must parse, got: {:?}", e);
}

/// `not` as a GATE primitive must STILL parse (the fix must not steal it).
#[test]
fn not_gate_primitive_still_parses() {
    let src = "module t; wire o; reg i; not g1(o, i); endmodule";
    let e = errors(src);
    assert!(e.is_empty(), "`not` gate must still parse, got: {:?}", e);
}

/// A plain (non-`not`) property must keep parsing.
#[test]
fn plain_property_still_parses() {
    let src = r#"
module m(input logic clk, rst, logic [3:0] a, b);
  ap: assert property ( @(posedge clk) (~rst && ($isunknown(|(a&b)))) );
endmodule
"#;
    assert!(errors(src).is_empty());
}
