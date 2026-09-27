//! The preprocessor copies ordinary text in bulk (comment stripping,
//! attribute stripping, macro expansion, line trimming). This pins its
//! output on the edge cases those fast paths skip over: comment
//! continuations, non-ASCII bytes in comments and strings, `(*` inside a
//! string, `@(*)` and `(**`, vertical-tab / form-feed / no-break-space
//! indentation before a directive, and an unterminated block comment.

use sv_parser::preprocessor::Preprocessor;

#[test]
fn bulk_copy_edge_cases_are_unchanged() {
    let src = "module m; // tail comment \\ \nwire a; /* block \u{e9} * not end \\\n still */ wire b;\n/* unterminated at eof? no */ string s = \"q\\\"\u{e9} (* not attr *) `X\";\n(* keep = 1 *) wire c; always @(*) d = e (** 2);\n\u{b}`define VTDEF 7\n\u{a0}`define NBDEF 8\n\u{c}  wire [`VTDEF:0] f = `NBDEF;\ninitial $display(\"`VTDEF %d\", `VTDEF); // \u{e9}\u{e9}\n   \t   \nendmodule /* eof";
    let expected = "module m;                 \\ \nwire a;                       \\\n          wire b;\n                              string s = \"q\\\"\u{c3}\u{a9} (* not attr *) `X\";\n               wire c; always @(*) d = e (** 2);\n\n\u{c2}\u{a0}`define NBDEF 8\n\u{c}  wire [7:0] f = `NBDEF;\ninitial $display(\"`VTDEF %d\", 7);        \n\nendmodule      f\n";
    let mut pp = Preprocessor::new();
    assert_eq!(pp.preprocess_file(src, None), expected);
}
