//! #283: IEEE 1800-2023 §11.13 allows a `let` in a subroutine and in a
//! sequential or parallel block (`block_item_declaration`, A.2.8). The
//! parser expands each instance in place: the let body with the actuals
//! substituted, parenthesized.

use sv_parser::ast::Description;
use sv_parser::ast::decl::ModuleItem;
use sv_parser::ast::expr::ExprKind;
use sv_parser::ast::stmt::StatementKind;
use sv_parser::parse;

#[test]
fn let_in_function_body_is_expanded() {
    let r = parse(
        r#"
module top;
  function automatic bit f(int x);
    let max(a, b = 2) = (a > b) ? a : b;
    return max(x) == 2;
  endfunction
  initial begin
    let one = 1;
    fork
      let two = 2;
      $display(one, two);
    join
  end
endmodule
"#,
    );
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let Description::Module(m) = &r.source.descriptions[0] else {
        panic!("module");
    };
    let f = m
        .items
        .iter()
        .find_map(|it| match it {
            ModuleItem::FunctionDeclaration(f) => Some(f),
            _ => None,
        })
        .expect("function f");
    // The let adds no statement; the return compares the expanded body.
    assert_eq!(f.items.len(), 1);
    let StatementKind::Return(Some(e)) = &f.items[0].kind else {
        panic!("return");
    };
    let ExprKind::Binary { left, .. } = &e.kind else {
        panic!("compare: {:?}", e.kind);
    };
    let ExprKind::Paren(body) = &left.kind else {
        panic!("expanded let: {:?}", left.kind);
    };
    let mut inner = &**body;
    while let ExprKind::Paren(i) = &inner.kind {
        inner = i;
    }
    assert!(
        matches!(inner.kind, ExprKind::Conditional { .. }),
        "{:?}",
        inner.kind
    );
    // In the fork, the let must not become a process of its own.
    let initial = m
        .items
        .iter()
        .find_map(|it| match it {
            ModuleItem::InitialConstruct(ic) => Some(ic),
            _ => None,
        })
        .expect("initial");
    let StatementKind::SeqBlock { stmts, .. } = &initial.stmt.kind else {
        panic!("block");
    };
    assert_eq!(stmts.len(), 1);
    let StatementKind::ParBlock { stmts: forked, .. } = &stmts[0].kind else {
        panic!("fork");
    };
    assert_eq!(forked.len(), 1);
}
