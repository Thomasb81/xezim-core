//! The lazy forms of an instantiated always block must agree with its
//! materialized tree: `PendingAlways::materialize_header` rewrites the
//! `@(...)` header exactly as `materialize` does (port substitution included),
//! `PendingAlways::scope` is the scope `materialize` records, and the
//! statement-level write census of the shared source, prefixed for the
//! instance, names what the census of the materialized tree names.

use xezim_core::ast::stmt::{EventControl, StatementKind, TimingControl};

fn elab_pending(src: &str) -> xezim_core::elaborate::ElaboratedModule {
    let (_, m) = xezim_core::parse_and_elaborate_multi(
        &[src.to_string()],
        Some("top"),
        &[],
        &["top.sv".to_string()],
        &[],
    )
    .expect("elaborate");
    m
}

fn header_terms(st: &xezim_core::ast::stmt::Statement) -> Vec<String> {
    let StatementKind::TimingControl {
        control: TimingControl::Event(EventControl::EventExpr(exprs)),
        ..
    } = &st.kind
    else {
        panic!("not an event-control header: {:?}", st.kind);
    };
    exprs
        .iter()
        .map(|e| format!("{:?} {:?}", e.edge, e.expr.kind))
        .collect()
}

const DESIGN: &str = r#"
module leaf(input clk, input rst, input [3:0] d, output reg [3:0] q);
  reg [3:0] r;
  always @(posedge clk or negedge rst) if (!rst) r <= 0; else r <= d;
  always @(posedge clk) q <= r;
endmodule
module top;
  logic [3:0] ck, d0;
  logic rst_n;
  wire [3:0] q0, q1;
  leaf u0(.clk(ck[1]), .rst(rst_n), .d(d0), .q(q0));
  genvar i;
  for (i = 0; i < 2; i++) begin : g
    leaf u(.clk(ck[i + 2]), .rst(rst_n), .d(d0 ^ i[3:0]), .q());
  end
endmodule
"#;

#[test]
fn header_and_scope_match_materialized_block() {
    let m = elab_pending(DESIGN);
    assert!(
        m.pending_always.len() >= 6,
        "expected the leaf always blocks to stay pending, got {}",
        m.pending_always.len()
    );
    for p in &m.pending_always {
        let header = p.materialize_header().expect("`@(...) body` shape");
        let full = p.clone().materialize();
        assert_eq!(header_terms(&header), header_terms(&full.stmt));
        assert_eq!(p.scope(), full.scope);
    }
}

#[test]
fn source_census_prefixes_to_the_materialized_census() {
    let m = elab_pending(DESIGN);
    for p in &m.pending_always {
        let (_, src_writes) = xezim_core::elaborate::stmt_write_targets(&p.source);
        let full = p.clone().materialize();
        let (_, writes) = xezim_core::elaborate::stmt_write_targets(&full.stmt);
        for w in &writes {
            assert!(
                src_writes
                    .iter()
                    .any(|s| *w == format!("{}{}", p.ctx.prefix, s) || w == s),
                "materialized write {w} not derivable from source writes {src_writes:?} under prefix {}",
                p.ctx.prefix
            );
        }
        assert_eq!(writes.len(), src_writes.len());
    }
}
