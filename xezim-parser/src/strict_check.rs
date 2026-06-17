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
    ModuleItem, PackageItem, ClassItem, ClassMethodKind,
    FunctionDeclaration, TaskDeclaration, FunctionPort,
};

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
    out
}

fn walk_module_items(items: &[ModuleItem], out: &mut Vec<String>) {
    for it in items {
        match it {
            ModuleItem::FunctionDeclaration(fd) => check_function(fd, out),
            ModuleItem::TaskDeclaration(td) => check_task(td, out),
            ModuleItem::ClassDeclaration(c) => walk_class_items(&c.items, out),
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
                _ => {}
            }
        }
    }
}

fn check_function(fd: &FunctionDeclaration, out: &mut Vec<String>) {
    check_dup_ports("function", &fd.name.name.name, &fd.ports, &fd.strict_body_ports, out);
}

fn check_task(td: &TaskDeclaration, out: &mut Vec<String>) {
    check_dup_ports("task", &td.name.name.name, &td.ports, &td.strict_body_ports, out);
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
    let names = ports.iter().map(|p| p.name.name.as_str())
        .chain(body_ports.iter().map(|i| i.name.as_str()));
    for n in names {
        if n.is_empty() { continue; }
        if seen.contains(&n) {
            out.push(format!("duplicate port '{}' in {} '{}'", n, kind, sub_name));
        } else {
            seen.push(n);
        }
    }
}
