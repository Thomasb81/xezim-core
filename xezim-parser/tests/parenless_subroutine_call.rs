//! §13.5 / A.6.9: a statement that is only a name is a subroutine call
//! written without parentheses. `u.t;`, `a.b.t;`, `p::t;`, `t;` and `c.m;`
//! must parse to the same zero-argument `Call` as `t();`, so later stages
//! treat them as calls. They used to stay bare reads: a hierarchical or
//! package-qualified task enabled this way inside a task was dropped, and
//! the calling `always` block never advanced time.

use sv_parser::ast::Description;
use sv_parser::ast::decl::ModuleItem;
use sv_parser::ast::expr::ExprKind;
use sv_parser::ast::stmt::StatementKind;
use sv_parser::parse;

#[test]
fn name_only_statements_parse_as_calls() {
    let r = parse(
        "module m;\n\
           task w;\n\
             u.t;\n\
             a.b.t;\n\
             p::t;\n\
             t;\n\
             c.m;\n\
             t();\n\
           endtask\n\
         endmodule\n",
    );
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let Description::Module(m) = &r.source.descriptions[0] else {
        panic!("expected a module");
    };
    let body = m
        .items
        .iter()
        .find_map(|it| match it {
            ModuleItem::TaskDeclaration(td) => Some(&td.items),
            _ => None,
        })
        .expect("task w");
    assert_eq!(body.len(), 6);
    for (i, s) in body.iter().enumerate() {
        let StatementKind::Expr(e) = &s.kind else {
            panic!("statement {i} is not an expression statement: {:?}", s.kind);
        };
        match &e.kind {
            ExprKind::Call { args, .. } => assert!(args.is_empty(), "statement {i}"),
            other => panic!("statement {i} is not a call: {other:?}"),
        }
    }
}
