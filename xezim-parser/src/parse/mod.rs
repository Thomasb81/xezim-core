//! Recursive descent parser for SystemVerilog IEEE 1800-2017/2023.

mod helpers;
mod types;
mod expressions;
mod statements;
mod declarations;
mod items;

use crate::ast::*;
use crate::ast::decl::{ModuleItem, PackageItem};
use crate::lexer::token::{Token, TokenKind};
use crate::diagnostics::Diagnostic;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    diagnostics: Vec<Diagnostic>,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0, diagnostics: Vec::new() }
    }

    pub fn diagnostics(&self) -> &[Diagnostic] { &self.diagnostics }

    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.severity == crate::diagnostics::Severity::Error)
    }

    /// source_text ::= { description }
    pub fn parse_source_text(&mut self) -> SourceText {
        let start = self.current().span.start;
        let mut descriptions = Vec::new();
        while !self.at(TokenKind::Eof) {
            if let Some(desc) = self.parse_description() {
                descriptions.push(desc);
            } else {
                self.error(format!("unexpected token: {:?}", self.current().text));
                self.bump();
            }
        }
        SourceText { descriptions, span: self.span_from(start) }
    }

    fn parse_description(&mut self) -> Option<Description> {
        match self.current_kind() {
            TokenKind::KwModule | TokenKind::KwMacromodule =>
                Some(Description::Module(self.parse_module_declaration())),
            TokenKind::KwInterface =>
                Some(Description::Interface(self.parse_interface_declaration())),
            TokenKind::KwProgram =>
                Some(Description::Program(self.parse_program_declaration())),
            TokenKind::KwPackage =>
                Some(Description::Package(self.parse_package_declaration())),
            TokenKind::KwNettype => {
                if let Some(ModuleItem::NettypeDeclaration(n)) = self.parse_module_item() {
                    Some(Description::PackageItem(PackageItem::Nettype(n)))
                } else { None }
            }
            TokenKind::KwClass =>
                Some(Description::Class(self.parse_class_declaration())),
            // IEEE 1800-2023 §19: a top-level (\$unit-scope) covergroup is
            // legal. We parse it for syntax acceptance but don't surface it
            // as a Description (no covergroup runtime hosted outside a
            // class/module). The body is fully consumed up through
            // `endgroup` plus optional label.
            TokenKind::KwCovergroup => {
                let _ = self.parse_covergroup_declaration();
                self.parse_description()
            }
            TokenKind::KwChecker => {
                if let Some(ModuleItem::CheckerDeclaration(c)) = self.parse_module_item() {
                    Some(Description::PackageItem(PackageItem::Checker(c)))
                } else { None }
            }
            TokenKind::KwVirtual if self.peek_kind() == TokenKind::KwClass =>
                Some(Description::Class(self.parse_class_declaration())),
            TokenKind::KwLet => {
                if let Some(ModuleItem::LetDeclaration(l)) = self.parse_module_item() {
                    Some(Description::PackageItem(PackageItem::Let(l)))
                } else { None }
            }
            TokenKind::KwTypedef =>
                Some(Description::TypedefDecl(self.parse_typedef_declaration())),
            TokenKind::KwImport => {
                if self.peek_kind() == TokenKind::StringLiteral {
                    Some(Description::DPIImport(self.parse_dpi_import()))
                } else {
                    Some(Description::ImportDecl(self.parse_import_declaration()))
                }
            }
            TokenKind::KwExport => {
                if self.peek_kind() == TokenKind::StringLiteral {
                    Some(Description::DPIExport(self.parse_dpi_export()))
                } else {
                    self.bump();
                    while !self.at(TokenKind::Semicolon) && !self.at(TokenKind::Eof) { self.bump(); }
                    self.expect(TokenKind::Semicolon);
                    self.parse_description()
                }
            }
            TokenKind::KwExtern => {
                self.bump();
                self.parse_description()
            }
            TokenKind::KwBind => {
                // IEEE 1800-2023 §23.11: `bind <target> <bind_mod> <inst>(<ports>);`
                // We parse the simple top-level form (no scope target list,
                // no instance-name selector) and surface a BindDirective so
                // elaboration can append the bound instantiation to every
                // instance of <target>. More elaborate selectors fall back
                // to parse-and-discard.
                let bind_start = self.current().span.start;
                self.bump();
                // Save position so we can fall back to the legacy skip-form
                // if the heuristic parse fails.
                let restart = self.pos;
                let target = self.parse_identifier();
                let bind_mod = self.parse_identifier();
                if self.at(TokenKind::Identifier) || self.at(TokenKind::EscapedIdentifier) {
                    let inst_start = self.current().span.start;
                    let iname = self.parse_identifier();
                    let dims = self.parse_unpacked_dimensions();
                    let conns = self.parse_port_connections();
                    if self.at(TokenKind::Semicolon) {
                        self.bump();
                        let span = self.span_from(bind_start);
                        let instance = crate::ast::decl::HierarchicalInstance {
                            name: iname,
                            dimensions: dims,
                            connections: conns,
                            span: self.span_from(inst_start),
                        };
                        let instantiation = crate::ast::decl::ModuleInstantiation {
                            module_name: bind_mod,
                            params: None,
                            instances: vec![instance],
                            span,
                        };
                        return Some(Description::Bind(
                            crate::ast::decl::BindDirective {
                                target_module: target,
                                instantiation,
                                span,
                            },
                        ));
                    }
                }
                // Heuristic parse failed (instance selector, multi-target,
                // unhandled syntax) — rewind and swallow until the next ';'.
                self.pos = restart;
                let mut depth_paren = 0i32;
                while !self.at(TokenKind::Eof) {
                    match self.current_kind() {
                        TokenKind::LParen => { depth_paren += 1; self.bump(); }
                        TokenKind::RParen => { depth_paren -= 1; self.bump(); }
                        TokenKind::Semicolon if depth_paren <= 0 => {
                            self.bump();
                            break;
                        }
                        _ => { self.bump(); }
                    }
                }
                self.parse_description()
            }
            TokenKind::KwConstraint => {
                // Out-of-class constraint definition at $unit scope:
                // `constraint ClassName::name { ... }[;]`. Parse and discard.
                self.bump();
                let _ = self.parse_hierarchical_identifier();
                if self.at(TokenKind::LBrace) {
                    self.bump();
                    let mut depth = 1;
                    while depth > 0 && !self.at(TokenKind::Eof) {
                        match self.current_kind() {
                            TokenKind::LBrace => depth += 1,
                            TokenKind::RBrace => depth -= 1,
                            _ => {}
                        }
                        self.bump();
                    }
                }
                if self.at(TokenKind::Semicolon) { self.bump(); }
                self.parse_description()
            }
            TokenKind::KwTimeunit | TokenKind::KwTimeprecision =>
                Some(Description::TimeunitsDecl(self.parse_timeunits_declaration())),
            TokenKind::KwFunction =>
                Some(Description::PackageItem(self.parse_package_item().unwrap())),
            TokenKind::KwTask =>
                Some(Description::PackageItem(self.parse_package_item().unwrap())),
            TokenKind::Directive => { self.bump(); self.parse_description() }
            _ => {
                // Top-level data declaration like `string label = "...";` —
                // xezim doesn't model $unit-scope vars, so skip past it.
                if self.is_data_type_keyword() || self.at(TokenKind::KwVar) || self.at(TokenKind::KwConst) {
                    let mut depth = 0i32;
                    while !self.at(TokenKind::Eof) {
                        match self.current_kind() {
                            TokenKind::LBrace | TokenKind::LParen | TokenKind::LBracket => depth += 1,
                            TokenKind::RBrace | TokenKind::RParen | TokenKind::RBracket => depth -= 1,
                            TokenKind::Semicolon if depth <= 0 => { self.bump(); break; }
                            _ => {}
                        }
                        self.bump();
                    }
                    return self.parse_description();
                }
                None
            }
        }
    }
}
