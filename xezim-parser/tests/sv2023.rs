//! Integration tests for SystemVerilog IEEE 1800-2023 parser support.
//!
//! Each test exercises a single SV-2023 gate: the lexer / preprocessor /
//! parser must accept the construct under SV-2023 and either reject it or
//! fall back to the SV-2017 interpretation when SV-2023 is not active.

use sv_parser::{parse, parse_with_std, SvStandard};
use sv_parser::ast::decl::ModuleItem;
use sv_parser::ast::types::PortDirection;
use sv_parser::ast::Description;
use sv_parser::lexer::{Lexer, token::TokenKind};
use sv_parser::preprocessor::Preprocessor;

// ---------------------------------------------------------------------------
// Triple-quoted string literals (IEEE 1800-2023 §5.9.1)
// ---------------------------------------------------------------------------

#[test]
fn triple_quoted_string_lexes_as_single_token_under_sv2023() {
    let src = "\"\"\"line1\nline2 with \"embedded\" quote\nline3\"\"\"";
    let toks = Lexer::with_standard(src, SvStandard::Sv2023).tokenize();
    // Expect exactly one StringLiteral token followed by Eof.
    let kinds: Vec<_> = toks.iter().map(|t| t.kind).collect();
    assert_eq!(kinds, vec![TokenKind::StringLiteral, TokenKind::Eof],
        "got token kinds {:?}", kinds);
    let lit = &toks[0];
    assert!(lit.text.starts_with("\"\"\""), "expected triple-quoted lexeme, got {:?}", lit.text);
    assert!(lit.text.ends_with("\"\"\""), "expected triple-quoted lexeme, got {:?}", lit.text);
    // The embedded "embedded" quote pair must be inside the single literal.
    assert!(lit.text.contains("embedded"));
}

#[test]
fn triple_quoted_string_is_not_recognized_under_sv2017() {
    // Under SV-2017 the leading `""` is an empty string followed by another
    // string literal. The lexer must NOT produce a single token covering the
    // whole input.
    let src = "\"\"\"hello\"\"\"";
    let toks = Lexer::with_standard(src, SvStandard::Sv2017).tokenize();
    let str_count = toks.iter().filter(|t| t.kind == TokenKind::StringLiteral).count();
    assert!(str_count >= 2,
        "SV-2017 should split the literal into multiple tokens, got {:?}",
        toks.iter().map(|t| (t.kind, t.text.clone())).collect::<Vec<_>>());
}

// ---------------------------------------------------------------------------
// `ref static` argument direction (IEEE 1800-2023 §13.5.2)
// ---------------------------------------------------------------------------

fn first_function_first_port_direction(src: &str, std: SvStandard) -> Option<PortDirection> {
    let result = parse_with_std(src, &[], &[], std);
    assert!(result.errors.is_empty(), "parse errors: {:?}", result.errors);
    for desc in &result.source.descriptions {
        if let Description::Module(m) = desc {
            for item in &m.items {
                if let ModuleItem::FunctionDeclaration(f) = item {
                    return f.ports.first().map(|p| p.direction);
                }
            }
        }
    }
    None
}

#[test]
fn ref_static_is_recognized_under_sv2023() {
    let src = "module m; function void f(ref static int a); endfunction endmodule";
    let dir = first_function_first_port_direction(src, SvStandard::Sv2023);
    assert_eq!(dir, Some(PortDirection::RefStatic),
        "expected RefStatic under SV-2023, got {:?}", dir);
}

#[test]
fn ref_keyword_alone_stays_as_ref_under_sv2023() {
    // `ref` not followed by `static` must remain `Ref`, not RefStatic.
    let src = "module m; function void f(ref int a); endfunction endmodule";
    let dir = first_function_first_port_direction(src, SvStandard::Sv2023);
    assert_eq!(dir, Some(PortDirection::Ref));
}

// ---------------------------------------------------------------------------
// `begin_keywords "1800-2023"` switches the active standard.
// ---------------------------------------------------------------------------

#[test]
fn begin_keywords_raises_standard_to_sv2023() {
    // Triple-quoted literal placed inside a `begin_keywords "1800-2023"`
    // region must lex as a single string even though the baseline
    // standard supplied to the parser is SV-2017.
    let src = "`begin_keywords \"1800-2023\"\n\
               module m;\n\
                 string s = \"\"\"multi\nline\"\"\";\n\
               endmodule\n\
               `end_keywords\n";
    let mut pp = Preprocessor::with_standard(SvStandard::Sv2017);
    let processed = pp.preprocess(src);
    // `live_standard()` restores after `end_keywords; `standard()`
    // remains at the peak so downstream lexers stay correctly configured.
    assert_eq!(pp.live_standard(), SvStandard::Sv2017,
        "live standard must be restored after `end_keywords (final = {:?})",
        pp.live_standard());
    assert_eq!(pp.standard(), SvStandard::Sv2023,
        "peak standard must reflect the elevated region");
    let toks = Lexer::with_standard(&processed, SvStandard::Sv2023).tokenize();
    let str_count = toks.iter().filter(|t| t.kind == TokenKind::StringLiteral).count();
    assert_eq!(str_count, 1,
        "triple-quoted string under `begin_keywords \"1800-2023\" should be \
         a single token, got tokens: {:?}",
        toks.iter().map(|t| (t.kind, t.text.clone())).collect::<Vec<_>>());
}

#[test]
fn begin_keywords_with_unknown_tag_is_ignored() {
    let mut pp = Preprocessor::with_standard(SvStandard::Sv2017);
    let _ = pp.preprocess("`begin_keywords \"9999-9999\"\nmodule m; endmodule\n`end_keywords\n");
    assert_eq!(pp.standard(), SvStandard::Sv2017,
        "unknown version tag must not change the active standard");
}

// ---------------------------------------------------------------------------
// `__FILE__ / `__LINE__ / `undefineall (IEEE 1800-2023 §22.5.2 / §22.13)
// ---------------------------------------------------------------------------

#[test]
fn line_macro_expands_to_current_line_number() {
    let src = "\n\n`__LINE__\n";
    let processed = Preprocessor::with_standard(SvStandard::Sv2023).preprocess(src);
    // The expanded literal "3" must appear on the third line of the output.
    let lines: Vec<&str> = processed.lines().collect();
    assert!(lines.len() >= 3, "expected at least 3 lines, got {:?}", processed);
    assert!(lines[2].trim() == "3",
        "expected `__LINE__ to expand to 3 on the third line, got {:?}",
        lines);
}

#[test]
fn file_macro_expands_to_unknown_when_no_path() {
    let processed = Preprocessor::with_standard(SvStandard::Sv2023)
        .preprocess("`__FILE__\n");
    assert!(processed.contains("\"<unknown>\""),
        "expected <unknown> placeholder when no file path, got {:?}", processed);
}

#[test]
fn undefineall_removes_user_macros() {
    let src = "`define FOO 1\n`define BAR 2\n`undefineall\n`FOO `BAR\n";
    let mut pp = Preprocessor::with_standard(SvStandard::Sv2023);
    let out = pp.preprocess(src);
    // After `undefineall, neither FOO nor BAR should expand.
    assert!(!pp.is_defined("FOO"), "FOO should be undefined after `undefineall");
    assert!(!pp.is_defined("BAR"), "BAR should be undefined after `undefineall");
    // The unexpanded tokens (or empty text) should be in the output, but the
    // numeric bodies "1" and "2" must NOT appear as standalone tokens.
    let trimmed: String = out.split_whitespace().collect();
    assert!(!trimmed.contains("12") && !trimmed.starts_with("1"),
        "macros should not have expanded after `undefineall, got {:?}", out);
}

#[test]
fn undefineall_preserves_sv_cov_builtins() {
    let mut pp = Preprocessor::with_standard(SvStandard::Sv2023);
    let _ = pp.preprocess("`undefineall\n");
    assert!(pp.is_defined("SV_COV_START"),
        "built-in SV_COV_START must survive `undefineall");
    assert!(pp.is_defined("SV_COV_ASSERTION"),
        "built-in SV_COV_ASSERTION must survive `undefineall");
}

// ---------------------------------------------------------------------------
// `default disable iff` (IEEE 1800-2023 §16.16.5)
// ---------------------------------------------------------------------------

#[test]
fn default_disable_iff_parses_under_sv2023() {
    let src = "module m(input logic clk, input logic rst);\n\
                 default disable iff (rst);\n\
               endmodule";
    let r = parse_with_std(src, &[], &[], SvStandard::Sv2023);
    assert!(r.errors.is_empty(), "parse errors: {:?}", r.errors);
    let mut saw = false;
    for desc in &r.source.descriptions {
        if let Description::Module(m) = desc {
            for item in &m.items {
                if let ModuleItem::DefaultDisableIff(_) = item {
                    saw = true;
                }
            }
        }
    }
    assert!(saw, "expected a DefaultDisableIff item in the module body");
}

#[test]
fn default_disable_iff_is_not_recognized_under_sv2017() {
    // Under SV-2017 the parser does not consume this form; the default
    // `parse()` (which uses SV-2017) should either skip it or report an
    // error — what it MUST NOT do is silently produce a DefaultDisableIff
    // ModuleItem.
    let src = "module m(input logic rst);\n\
                 default disable iff (rst);\n\
               endmodule";
    let r = parse(src);
    for desc in &r.source.descriptions {
        if let Description::Module(m) = desc {
            for item in &m.items {
                if let ModuleItem::DefaultDisableIff(_) = item {
                    panic!("DefaultDisableIff must not be emitted under SV-2017");
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SvStandard helpers
// ---------------------------------------------------------------------------

#[test]
fn standard_version_string_parsing() {
    assert_eq!(SvStandard::from_version_string("\"1800-2023\""), Some(SvStandard::Sv2023));
    assert_eq!(SvStandard::from_version_string("1800-2017"),      Some(SvStandard::Sv2017));
    assert_eq!(SvStandard::from_version_string("1800-2012"),      Some(SvStandard::Sv2017));
    assert_eq!(SvStandard::from_version_string("not-a-version"),  None);
}

#[test]
fn default_standard_is_sv2017() {
    assert_eq!(SvStandard::DEFAULT, SvStandard::Sv2017);
    assert!(!SvStandard::Sv2017.is_2023_or_later());
    assert!(SvStandard::Sv2023.is_2023_or_later());
}

// ---------------------------------------------------------------------------
// `randsequence` statement
// ---------------------------------------------------------------------------

#[test]
fn randsequence_parses_without_errors() {
    let src = "module m;\n\
                 initial begin\n\
                   randsequence (main)\n\
                     main : first second ;\n\
                     first : { $display(\"first\"); } ;\n\
                     second : { $display(\"second\"); } ;\n\
                   endsequence\n\
                 end\n\
               endmodule";
    let r = parse_with_std(src, &[], &[], SvStandard::Sv2023);
    assert!(r.errors.is_empty(), "parse errors: {:?}", r.errors);
}

// ---------------------------------------------------------------------------
// `pragma protect` envelope skipping
// ---------------------------------------------------------------------------

#[test]
fn pragma_protect_envelope_is_dropped() {
    // The garbage between begin_protected / end_protected must not reach
    // the lexer. If it did, `xyz \x01\x02` would explode into Unknown
    // tokens and the surrounding `module / endmodule pair would not pair up.
    let src = "module m;\n\
               `pragma protect begin_protected\n\
                 this is not legal SV at all !!!! @#$\n\
                 \"\"unterminated and weird\n\
               `pragma protect end_protected\n\
               endmodule";
    let r = parse_with_std(src, &[], &[], SvStandard::Sv2023);
    assert!(r.errors.is_empty(),
        "pragma-protect envelope must not surface errors: {:?}",
        r.errors);
}

// ---------------------------------------------------------------------------
// `union soft packed` — IEEE 1800-2023 §7.3.2
// ---------------------------------------------------------------------------

fn first_data_decl_type(src: &str, std: SvStandard) -> sv_parser::ast::types::DataType {
    let r = parse_with_std(src, &[], &[], std);
    assert!(r.errors.is_empty(), "parse errors: {:?}", r.errors);
    for desc in r.source.descriptions {
        if let Description::Module(m) = desc {
            for item in m.items {
                if let ModuleItem::DataDeclaration(dd) = item {
                    return dd.data_type;
                }
            }
        }
    }
    panic!("no data declaration found in source");
}

#[test]
fn union_soft_packed_parses_and_sets_soft_flag() {
    // Members have different widths — this is the precise SV-2023 use
    // case: a regular `union packed` would reject it.
    let src = "module m;\n\
                 union soft packed {\n\
                   bit [7:0]  a;\n\
                   bit [15:0] b;\n\
                   bit [31:0] c;\n\
                 } u;\n\
               endmodule";
    let dt = first_data_decl_type(src, SvStandard::Sv2023);
    if let sv_parser::ast::types::DataType::Struct(su) = dt {
        assert_eq!(su.kind, sv_parser::ast::types::StructUnionKind::Union);
        assert!(su.soft, "soft must be true for `union soft packed`");
        assert!(su.packed, "packed must be true");
        assert_eq!(su.members.len(), 3);
    } else {
        panic!("expected a Struct/Union data type");
    }
}

#[test]
fn plain_union_packed_keeps_soft_false() {
    let src = "module m;\n\
                 union packed {\n\
                   bit [7:0] a;\n\
                   bit [7:0] b;\n\
                 } u;\n\
               endmodule";
    let dt = first_data_decl_type(src, SvStandard::Sv2023);
    if let sv_parser::ast::types::DataType::Struct(su) = dt {
        assert!(!su.soft, "plain union packed must leave soft = false");
        assert!(su.packed);
    } else { panic!("expected union type"); }
}

#[test]
fn union_soft_packed_is_not_recognized_under_sv2017() {
    // Under SV-2017 the `soft` keyword between `union` and `packed` must
    // not be consumed as a union modifier. Either a parse error surfaces
    // or `soft` is treated as something else, but the soft flag on the
    // AST must NEVER be set.
    let src = "module m;\n\
                 union soft packed {\n\
                   bit [7:0] a;\n\
                 } u;\n\
               endmodule";
    let r = parse_with_std(src, &[], &[], SvStandard::Sv2017);
    for desc in &r.source.descriptions {
        if let Description::Module(m) = desc {
            for item in &m.items {
                if let ModuleItem::DataDeclaration(dd) = item {
                    if let sv_parser::ast::types::DataType::Struct(su) = &dd.data_type {
                        assert!(!su.soft,
                            "soft flag must NEVER be set under SV-2017");
                    }
                }
            }
        }
    }
}

#[test]
fn soft_modifier_does_not_apply_to_structs() {
    // The `soft` modifier is only legal on unions; a struct following
    // `soft` must still parse, with soft = false.
    let src = "module m;\n\
                 struct packed { bit [7:0] a; bit [7:0] b; } s;\n\
               endmodule";
    let dt = first_data_decl_type(src, SvStandard::Sv2023);
    if let sv_parser::ast::types::DataType::Struct(su) = dt {
        assert_eq!(su.kind, sv_parser::ast::types::StructUnionKind::Struct);
        assert!(!su.soft, "soft must always be false for structs");
    } else { panic!("expected Struct type"); }
}

// ---------------------------------------------------------------------------
// Real-valued coverpoints (IEEE 1800-2023 §19.5)
// ---------------------------------------------------------------------------

fn first_covergroup(src: &str, std: SvStandard) -> sv_parser::ast::decl::CovergroupDeclaration {
    let r = parse_with_std(src, &[], &[], std);
    assert!(r.errors.is_empty(), "parse errors: {:?}", r.errors);
    for desc in r.source.descriptions {
        if let Description::Module(m) = desc {
            for item in m.items {
                if let ModuleItem::CovergroupDeclaration(c) = item {
                    return c;
                }
            }
        }
    }
    panic!("no covergroup declaration found");
}

#[test]
fn covergroup_with_function_sample_parses() {
    let src = "module m;\n\
                 covergroup cg with function sample (real x);\n\
                   cp: coverpoint x;\n\
                 endgroup\n\
               endmodule";
    let cg = first_covergroup(src, SvStandard::Sv2023);
    assert!(cg.sample_args.is_some(),
        "with function sample(...) should populate sample_args");
    let args = cg.sample_args.unwrap();
    assert!(args.contains("real"), "sample arg list should mention `real`, got {:?}", args);
    assert!(args.contains("x"));
}

#[test]
fn coverpoint_real_keyword_marks_is_real() {
    let src = "module m;\n\
                 covergroup cg with function sample (real x);\n\
                   cp: coverpoint real x { bins lo = { [0.0:1.0] }; }\n\
                 endgroup\n\
               endmodule";
    let cg = first_covergroup(src, SvStandard::Sv2023);
    let mut saw_real_cp = false;
    for item in &cg.items {
        if let sv_parser::ast::decl::CovergroupItem::Coverpoint(cp) = item {
            saw_real_cp = cp.is_real;
        }
    }
    assert!(saw_real_cp,
        "coverpoint with `real` type keyword must set is_real=true");
}

#[test]
fn coverpoint_real_is_not_recognized_under_sv2017() {
    // Under SV-2017 the keyword `real` after `coverpoint` is not allowed;
    // the parser must NOT silently produce a coverpoint with is_real set.
    let src = "module m;\n\
                 covergroup cg with function sample (real x);\n\
                   cp: coverpoint real x;\n\
                 endgroup\n\
               endmodule";
    let r = parse_with_std(src, &[], &[], SvStandard::Sv2017);
    // Either an error or the coverpoint isn't marked real — both are
    // acceptable as SV-2017 behaviour. What is NOT acceptable is is_real=true.
    for desc in &r.source.descriptions {
        if let Description::Module(m) = desc {
            for item in &m.items {
                if let ModuleItem::CovergroupDeclaration(cg) = item {
                    for cgi in &cg.items {
                        if let sv_parser::ast::decl::CovergroupItem::Coverpoint(cp) = cgi {
                            assert!(!cp.is_real,
                                "is_real must not be set under SV-2017");
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Property / sequence bodies (initial pass — disable iff + raw body text)
// ---------------------------------------------------------------------------

fn first_property<'a>(src: &'a str, std: SvStandard) -> sv_parser::ast::decl::PropertyDeclaration {
    let r = parse_with_std(src, &[], &[], std);
    assert!(r.errors.is_empty(), "parse errors: {:?}", r.errors);
    for desc in r.source.descriptions {
        if let Description::Module(m) = desc {
            for item in m.items {
                if let ModuleItem::PropertyDeclaration(p) = item {
                    return p;
                }
            }
        }
    }
    panic!("no property declaration in source");
}

#[test]
fn property_body_text_is_captured() {
    let src = "module m;\n\
                 property p1;\n\
                   @(posedge clk) a |-> b;\n\
                 endproperty\n\
               endmodule";
    let p = first_property(src, SvStandard::Sv2023);
    assert_eq!(p.name.name, "p1");
    assert!(p.body_text.contains("|->"),
        "expected `|->` in captured body, got {:?}", p.body_text);
    assert!(p.disable_iff.is_none(), "no disable iff was given");
}

#[test]
fn property_disable_iff_prefix_is_parsed() {
    let src = "module m;\n\
                 property p1;\n\
                   disable iff (rst) @(posedge clk) a |-> b;\n\
                 endproperty\n\
               endmodule";
    let p = first_property(src, SvStandard::Sv2023);
    assert!(p.disable_iff.is_some(),
        "disable iff prefix should populate disable_iff");
}

#[test]
fn sequence_body_text_is_captured() {
    let src = "module m;\n\
                 sequence s1;\n\
                   a ##1 b ##2 c;\n\
                 endsequence\n\
               endmodule";
    let r = parse_with_std(src, &[], &[], SvStandard::Sv2023);
    assert!(r.errors.is_empty(), "parse errors: {:?}", r.errors);
    let mut saw = false;
    for desc in &r.source.descriptions {
        if let Description::Module(m) = desc {
            for item in &m.items {
                if let ModuleItem::SequenceDeclaration(s) = item {
                    assert_eq!(s.name.name, "s1");
                    // Tokens are space-joined in body_text, so `##1`
                    // round-trips as `## 1`.
                    assert!(s.body_text.contains("##"),
                        "expected `##` cycle-delay in captured sequence body, got {:?}",
                        s.body_text);
                    saw = true;
                }
            }
        }
    }
    assert!(saw, "no sequence declaration found");
}

// ---------------------------------------------------------------------------
// Extern / pure constraints (IEEE 1800-2023 §18.5.13)
// ---------------------------------------------------------------------------

fn class_constraint<'a>(src: &'a str, std: SvStandard) -> sv_parser::ast::decl::ClassConstraint {
    let r = parse_with_std(src, &[], &[], std);
    assert!(r.errors.is_empty(), "parse errors: {:?}", r.errors);
    for desc in r.source.descriptions {
        if let Description::Class(c) = desc {
            for item in c.items {
                if let sv_parser::ast::decl::ClassItem::Constraint(cc) = item {
                    return cc;
                }
            }
        }
    }
    panic!("no class-level constraint found in source");
}

#[test]
fn extern_constraint_in_class_parses_with_no_body() {
    let src = "class C; extern constraint c1; endclass";
    let cc = class_constraint(src, SvStandard::Sv2023);
    assert!(cc.is_extern, "extern constraint must set is_extern");
    assert!(!cc.has_body, "extern constraint must have no body");
    assert_eq!(cc.name.name, "c1");
}

#[test]
fn pure_constraint_in_interface_class_parses() {
    // IEEE 1800-2023 §18.5.13: `pure constraint cname;` in an interface
    // class declares a constraint that derived classes must override.
    let src = "interface class IC; pure constraint c1; endclass";
    let cc = class_constraint(src, SvStandard::Sv2023);
    assert!(cc.is_pure, "pure constraint must set is_pure");
    assert!(!cc.has_body);
}

#[test]
fn out_of_class_constraint_definition_at_module_scope_parses() {
    let src = "module m; constraint C::c1 { x inside { [0:7] }; } endmodule";
    let r = parse_with_std(src, &[], &[], SvStandard::Sv2023);
    assert!(r.errors.is_empty(), "parse errors: {:?}", r.errors);
    let mut saw = false;
    for desc in &r.source.descriptions {
        if let Description::Module(m) = desc {
            for item in &m.items {
                if let ModuleItem::OutOfClassConstraint { class_name, constraint_name } = item {
                    assert_eq!(class_name, "C");
                    assert_eq!(constraint_name, "c1");
                    saw = true;
                }
            }
        }
    }
    assert!(saw, "expected OutOfClassConstraint at module scope");
}

// ---------------------------------------------------------------------------
// Streaming concatenation (IEEE 1800-2023 §11.4.14)
// ---------------------------------------------------------------------------

#[test]
fn streaming_concat_pack_form_parses() {
    let src = "module m;\n\
                 logic [31:0] packed_w;\n\
                 logic [7:0] a, b, c, d;\n\
                 assign packed_w = {>>{a, b, c, d}};\n\
               endmodule";
    let r = parse_with_std(src, &[], &[], SvStandard::Sv2023);
    assert!(r.errors.is_empty(), "parse errors: {:?}", r.errors);
}

#[test]
fn streaming_concat_with_byte_slice_parses() {
    let src = "module m;\n\
                 logic [31:0] w;\n\
                 logic [31:0] r;\n\
                 assign r = {<<byte{w}};\n\
               endmodule";
    let r = parse_with_std(src, &[], &[], SvStandard::Sv2023);
    assert!(r.errors.is_empty(), "parse errors: {:?}", r.errors);
}

#[test]
fn streaming_concat_into_dynamic_array_parses() {
    // Streaming with a dynamic-array source expression on the RHS — the
    // SV-2023 case that motivated the gap-analysis bullet.
    let src = "module m;\n\
                 bit [7:0] arr [];\n\
                 bit [31:0] w;\n\
                 initial begin\n\
                   arr = new[4];\n\
                   {>>{arr}} = w;\n\
                 end\n\
               endmodule";
    let r = parse_with_std(src, &[], &[], SvStandard::Sv2023);
    assert!(r.errors.is_empty(), "parse errors: {:?}", r.errors);
}

#[test]
fn pragma_protect_short_form_envelope_is_dropped() {
    // `pragma protect begin / `pragma protect end short form.
    let src = "module m;\n\
               `pragma protect begin\n\
                 zzzz !!! unparsable\n\
               `pragma protect end\n\
               endmodule";
    let r = parse_with_std(src, &[], &[], SvStandard::Sv2023);
    assert!(r.errors.is_empty(),
        "short-form pragma-protect envelope must not surface errors: {:?}",
        r.errors);
}
