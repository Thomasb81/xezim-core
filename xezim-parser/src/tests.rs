use crate::ast::{Description, decl::ModuleItem};
use crate::parse;

#[test]
fn test_function_ports_implicit_packed() {
    let source = "module m; function void f(input [7:0] a); endfunction endmodule";
    let result = parse(source);
    assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);
}

#[test]
fn test_top_level_function() {
    let source = "function void f(input [7:0] a); endfunction";
    let result = parse(source);
    assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);
}

#[test]
fn test_begin_at_module_level() {
    let source = "module m; begin wire a; end endmodule";
    let result = parse(source);
    assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);
}

#[test]
fn test_assignment_pattern() {
    let source = "module m; initial pair = '{a:4'hA, b:4'h5}; endmodule";
    let result = parse(source);
    assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);
}

#[test]
fn test_associative_arrays() {
    let source = "module m; int aa [string]; int aa2 [*]; endmodule";
    let result = parse(source);
    assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);
}

#[test]
fn test_queues() {
    let source = "module m; int q [$]; int q2 [$:255]; endmodule";
    let result = parse(source);
    assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);
}

#[test]
fn test_interfaces_modports() {
    let source = "
        interface req_gnt_if;
          logic req;
          logic gnt;
          modport master (output req, input gnt);
          modport slave  (input req, output gnt);
        endinterface

        module req_master(req_gnt_if.master bus);
          initial bus.req = 1'b1;
        endmodule

        module req_slave(req_gnt_if.slave bus);
          always @(*) bus.gnt = bus.req;
        endmodule
    ";
    let result = parse(source);
    assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);
}

#[test]
fn test_module_header_imports() {
    let source = "
        package p;
        endpackage

        module m import p::*; (input logic a);
          initial a = 1'b0;
        endmodule
    ";
    let result = parse(source);
    assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);

    let module = result
        .source
        .descriptions
        .iter()
        .find_map(|desc| match desc {
            Description::Module(module) if module.name.name == "m" => Some(module),
            _ => None,
        })
        .expect("module m not found");

    match module.items.first() {
        Some(ModuleItem::ImportDeclaration(import)) => {
            assert_eq!(import.items.len(), 1);
            assert_eq!(import.items[0].package.name, "p");
            assert!(import.items[0].item.is_none());
        }
        other => panic!(
            "expected header import as first module item, got {:?}",
            other
        ),
    }
}

#[test]
fn test_numeric_size_cast_expression() {
    let source = "module m; logic [31:0] x; initial x = 32'(1); endmodule";
    let result = parse(source);
    assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);
}

#[test]
fn test_scoped_constant_unpacked_dimension() {
    let source = "
        package ibex_pkg;
          localparam int IC_NUM_WAYS = 4;
        endpackage

        module m;
          logic [7:0] a [ibex_pkg::IC_NUM_WAYS-1:0];
        endmodule
    ";
    let result = parse(source);
    assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);
}

#[test]
fn test_processes_events() {
    let source = "
        module test;
          event ev;
          initial begin
            -> ev;
            ->> ev;
            @ev;
          end
        endmodule
    ";
    let result = parse(source);
    assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);
}

#[test]
fn test_semaphores() {
    let source = "
        module test;
          semaphore sem;
          initial begin
            sem = new(1);
            sem.get(1);
            sem.put(1);
          end
        endmodule
    ";
    let result = parse(source);
    assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);
}

/// §31 timing checks: edge controls (keyword and descriptor list), `&&&`
/// conditions, omitted arguments, and a specify-block `specparam` hoisted
/// to a module-level localparam.
#[test]
fn test_specify_timing_checks() {
    use crate::ast::decl::{TIMING_NEGEDGE, TIMING_POSEDGE, timing_edge_bit};
    let source = "
        module ff(input d, input clk, input en);
          reg n;
          specify
            specparam tSU = 1.5;
            $setuphold(posedge clk &&& en, d, tSU, 1, n, , , dclk, dd);
            $width(negedge clk, 2);
            $setup(d, edge[01, x1] clk, 3);
          endspecify
        endmodule
    ";
    let result = parse(source);
    assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);
    let Some(Description::Module(m)) = result.source.descriptions.first() else {
        panic!("module expected");
    };
    assert!(
        m.items
            .iter()
            .any(|i| matches!(i, ModuleItem::LocalparamDeclaration(_)))
    );
    let sb = m
        .items
        .iter()
        .find_map(|i| match i {
            ModuleItem::SpecifyBlock(sb) => Some(sb),
            _ => None,
        })
        .expect("specify block");
    let tc = &sb.timing_checks;
    assert_eq!(tc.len(), 3);
    assert_eq!(tc[0].name, "$setuphold");
    assert_eq!(tc[0].args.len(), 9);
    let r = tc[0].args[0].as_ref().unwrap();
    assert_eq!(r.edges, Some(TIMING_POSEDGE));
    assert!(r.cond.is_some());
    assert_eq!(r.text, "posedge clk &&& en");
    assert!(tc[0].args[5].is_none() && tc[0].args[6].is_none());
    assert_eq!(sb.delayed_nets.len(), 2);
    assert_eq!(tc[1].args[0].as_ref().unwrap().edges, Some(TIMING_NEGEDGE));
    assert_eq!(
        tc[2].args[1].as_ref().unwrap().edges,
        Some(timing_edge_bit(0, 1) | timing_edge_bit(2, 1))
    );
}
