//! Strict negative-test checks — a *second* validation pass that runs
//! ALONGSIDE the permissive main parser without modifying it. The main parser
//! deliberately accepts/recovers from many LRM-illegal constructs to maximize
//! pass rate on valid designs. This pass walks the parsed AST and reports
//! violations a conformance checker must diagnose.
//!
//! Gated by [`crate::strict_checks`] (the `--strict` switch, on by default;
//! `--no-strict` disables it). It runs on EVERY source, so each check must be
//! precise — a false positive rejects a valid design. Working on the AST (not
//! the token stream) gives each task/function/etc. a well-scoped node, so
//! checks don't suffer the scope ambiguity a token scan does (e.g. DPI/extern
//! functions are distinct nodes, not open scopes).

use crate::ast::Description;
use crate::ast::decl::{
    ClassItem, ClassMethodKind, FunctionDeclaration, FunctionPort, ModuleItem, PackageItem,
    ParamConnection, ParameterDeclaration, ParameterKind, TaskDeclaration,
};
use crate::ast::stmt::{Statement, StatementKind};
use std::collections::{HashMap, HashSet};

/// Run all enabled strict checks over one file's parsed descriptions. Returns
/// human-readable violation messages (empty = clean). No-op when disabled.
pub fn strict_violations(descriptions: &[Description]) -> Vec<String> {
    if !crate::strict_checks() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for d in descriptions {
        match d {
            Description::Module(m) => walk_module_items(&m.items, &mut out),
            Description::Program(p) => walk_module_items(&p.items, &mut out),
            Description::Interface(i) => walk_module_items(&i.items, &mut out),
            Description::Package(p) => walk_package_items(&p.items, &mut out),
            Description::Class(c) => walk_class_items(&c.items, &mut out),
            Description::PackageItem(pi) => walk_package_item(pi, &mut out),
            _ => {}
        }
    }
    // §6.20.2 / §23.10: a named parameter override must name an *overridable*
    // parameter (not a localparam, and it must exist). Built from the modules
    // declared in THIS file; instantiations of modules defined elsewhere are
    // skipped (can't resolve, so no false positive).
    let overridable = build_module_overridable_params(descriptions);
    for d in descriptions {
        if let Description::Module(m) = d {
            check_param_overrides(&m.items, &overridable, &mut out);
        }
    }
    out
}

/// name -> set of overridable parameter names for every module in this file.
/// "Overridable" excludes localparams (header `localparam` / body
/// `localparam`); both header `#(...)` and body `parameter` decls are included.
fn build_module_overridable_params(
    descriptions: &[Description],
) -> HashMap<String, HashSet<String>> {
    let mut map = HashMap::new();
    for d in descriptions {
        if let Description::Module(m) = d {
            let mut params = HashSet::new();
            for pd in &m.params {
                add_overridable_param_names(pd, &mut params);
            }
            for it in &m.items {
                if let ModuleItem::ParameterDeclaration(pd) = it {
                    add_overridable_param_names(pd, &mut params);
                }
            }
            map.insert(m.name.name.clone(), params);
        }
    }
    map
}

fn add_overridable_param_names(pd: &ParameterDeclaration, out: &mut HashSet<String>) {
    if pd.local {
        return; // localparam — not overridable
    }
    match &pd.kind {
        ParameterKind::Data { assignments, .. } => {
            for a in assignments {
                out.insert(a.name.name.clone());
            }
        }
        ParameterKind::Type { assignments } => {
            for a in assignments {
                out.insert(a.name.name.clone());
            }
        }
    }
}

fn check_param_overrides(
    items: &[ModuleItem],
    overridable: &HashMap<String, HashSet<String>>,
    out: &mut Vec<String>,
) {
    for it in items {
        if let ModuleItem::ModuleInstantiation(inst) = it {
            // Only check when the target module is defined in this file.
            let Some(params) = overridable.get(&inst.module_name.name) else {
                continue;
            };
            if let Some(conns) = &inst.params {
                for c in conns {
                    if let ParamConnection::Named { name, .. } = c {
                        if !params.contains(&name.name) {
                            out.push(format!(
                                "cannot override '{}' of module '{}' — not an overridable parameter",
                                name.name, inst.module_name.name
                            ));
                        }
                    }
                }
            }
        }
    }
}

fn walk_module_items(items: &[ModuleItem], out: &mut Vec<String>) {
    for it in items {
        match it {
            ModuleItem::FunctionDeclaration(fd) => check_function(fd, out),
            ModuleItem::TaskDeclaration(td) => check_task(td, out),
            ModuleItem::ClassDeclaration(c) => walk_class_items(&c.items, out),
            ModuleItem::InitialConstruct(ic) => check_decl_order_stmt(&ic.stmt, out),
            ModuleItem::AlwaysConstruct(ac) => check_decl_order_stmt(&ac.stmt, out),
            ModuleItem::FinalConstruct(fc) => check_decl_order_stmt(&fc.stmt, out),
            ModuleItem::GenerateRegion(g) => walk_module_items(&g.items, out),
            ModuleItem::GenerateFor(g) => walk_module_items(&g.items, out),
            ModuleItem::GenerateIf(g) => {
                for (_, items) in &g.branches {
                    walk_module_items(items, out);
                }
            }
            ModuleItem::GenerateCase(g) => {
                for arm in &g.arms {
                    walk_module_items(&arm.items, out);
                }
            }
            _ => {}
        }
    }
}

fn walk_package_items(items: &[PackageItem], out: &mut Vec<String>) {
    for it in items {
        walk_package_item(it, out);
    }
}

fn walk_package_item(it: &PackageItem, out: &mut Vec<String>) {
    match it {
        PackageItem::Function(fd) => check_function(fd, out),
        PackageItem::Task(td) => check_task(td, out),
        PackageItem::Class(c) => walk_class_items(&c.items, out),
        _ => {}
    }
}

fn walk_class_items(items: &[ClassItem], out: &mut Vec<String>) {
    for it in items {
        if let ClassItem::Method(m) = it {
            match &m.kind {
                ClassMethodKind::Function(fd)
                | ClassMethodKind::PureVirtual(fd)
                | ClassMethodKind::Extern(fd) => check_function(fd, out),
                ClassMethodKind::Task(td) => check_task(td, out),
            }
        }
    }
}

fn check_function(fd: &FunctionDeclaration, out: &mut Vec<String>) {
    check_dup_ports(
        "function",
        &fd.name.name.name,
        &fd.ports,
        &fd.strict_body_ports,
        out,
    );
    check_decl_order_list(&fd.items, out);
}

fn check_task(td: &TaskDeclaration, out: &mut Vec<String>) {
    check_dup_ports(
        "task",
        &td.name.name.name,
        &td.ports,
        &td.strict_body_ports,
        out,
    );
    check_decl_order_list(&td.items, out);
}

/// §9.3.1 / §9.3.2 / §13: in a `begin`-`end` or `fork`-`join` block and in a
/// subroutine body, every block item declaration precedes the first
/// statement. A null statement does not end the declaration region here (a
/// stray `;` between declarations is common and harmless), nor do the
/// parser's internal lowering nodes.
fn check_decl_order_list(stmts: &[Statement], out: &mut Vec<String>) {
    let mut seen_stmt = false;
    for st in stmts {
        match &st.kind {
            StatementKind::VarDecl { declarators, .. } => {
                if seen_stmt {
                    let name = declarators
                        .first()
                        .map(|d| d.name.name.as_str())
                        .unwrap_or("");
                    out.push(format!(
                        "declaration of '{name}' follows a statement; block item declarations must precede the statements of a block (§9.3.1)"
                    ));
                }
            }
            StatementKind::Typedef(_) => {
                if seen_stmt {
                    out.push(
                        "typedef follows a statement; block item declarations must precede the statements of a block (§9.3.1)"
                            .to_string(),
                    );
                }
            }
            StatementKind::Null
            | StatementKind::ScopePop
            | StatementKind::LoopStep
            | StatementKind::ForeachTail { .. }
            | StatementKind::ForeverTail { .. } => {}
            _ => seen_stmt = true,
        }
        check_decl_order_stmt(st, out);
    }
}

fn check_decl_order_stmt(st: &Statement, out: &mut Vec<String>) {
    match &st.kind {
        StatementKind::SeqBlock { stmts, .. } | StatementKind::ParBlock { stmts, .. } => {
            check_decl_order_list(stmts, out)
        }
        StatementKind::If {
            then_stmt,
            else_stmt,
            ..
        } => {
            check_decl_order_stmt(then_stmt, out);
            if let Some(e) = else_stmt {
                check_decl_order_stmt(e, out);
            }
        }
        StatementKind::Case { items, .. } => {
            for it in items {
                check_decl_order_stmt(&it.stmt, out);
            }
        }
        StatementKind::For { body, .. }
        | StatementKind::Foreach { body, .. }
        | StatementKind::While { body, .. }
        | StatementKind::DoWhile { body, .. }
        | StatementKind::Repeat { body, .. }
        | StatementKind::Forever { body } => check_decl_order_stmt(body, out),
        StatementKind::TimingControl { stmt, .. } | StatementKind::Wait { stmt, .. } => {
            check_decl_order_stmt(stmt, out)
        }
        _ => {}
    }
}

/// §13.3/§13.4: a subroutine must not declare the same port twice. Combines the
/// ANSI port list with the retained non-ANSI body declarations.
fn check_dup_ports(
    kind: &str,
    sub_name: &str,
    ports: &[FunctionPort],
    body_ports: &[crate::ast::Identifier],
    out: &mut Vec<String>,
) {
    let mut seen: Vec<&str> = Vec::new();
    let names = ports
        .iter()
        .map(|p| p.name.name.as_str())
        .chain(body_ports.iter().map(|i| i.name.as_str()));
    for n in names {
        if n.is_empty() {
            continue;
        }
        if seen.contains(&n) {
            out.push(format!("duplicate port '{}' in {} '{}'", n, kind, sub_name));
        } else {
            seen.push(n);
        }
    }
}
