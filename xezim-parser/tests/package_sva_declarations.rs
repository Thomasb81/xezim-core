//! §26.2: package_item includes concurrent_assertion_item_declaration, so a
//! package may declare named properties and sequences. The package-item
//! parser had no arm for `property`/`sequence`, read `property` as a type
//! name, and every token after it derailed the whole package.

use sv_parser::parse;

#[test]
fn property_with_ports_parses_in_a_package() {
    let src = r#"
package chk_pkg;
  property no_overlap(clk, arm, req_a, req_b);
    @(posedge clk) not(arm && (req_a && req_b || $isunknown(req_a) || $isunknown(req_b)));
  endproperty
endpackage
module m; endmodule
"#;
    let result = parse(src);
    assert!(result.errors.is_empty(), "parse errors: {:?}", result.errors);
}

#[test]
fn portless_property_and_sequence_parse_in_a_package() {
    let src = r#"
package chk_pkg;
  property always_true;
    1;
  endproperty
  sequence s_pulse;
    @(posedge clk) 1;
  endsequence
endpackage
module m; endmodule
"#;
    let result = parse(src);
    assert!(result.errors.is_empty(), "parse errors: {:?}", result.errors);
}
