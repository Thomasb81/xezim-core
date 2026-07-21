//! xezim-core: shared SystemVerilog elaboration, runtime primitives, and
//! artifact format used by both the `xezim` bytecode interpreter and the
//! `xezim-b` native compiler.

pub mod value;
pub mod bits2;
pub mod elaborate;
pub mod sdf;
pub mod vcd_sink;
pub mod stdout_sink;

/// Deterministic hasher for `HashMap`/`HashSet` so iteration order is
/// reproducible across runs. Both `crate::hasher::HashMap` (default `RandomState`)
/// and `std::collections::HashMap` use OS-random seeds, which causes
/// non-deterministic iteration. For simulator correctness debugging
/// (and to make c910 memcpy reproducible at the same cycle each run),
/// we use a fixed-seeded ahash state.
pub mod hasher {
    #[derive(Clone, Debug)]
    pub struct DeterministicState(ahash::RandomState);

    impl Default for DeterministicState {
        fn default() -> Self {
            DeterministicState(ahash::RandomState::with_seeds(
                0xdead_beef_cafe_babe,
                0xfeed_face_0123_4567,
                0xbada_55_b01d_face,
                0x0123_4567_89ab_cdef,
            ))
        }
    }

    impl std::hash::BuildHasher for DeterministicState {
        type Hasher = ahash::AHasher;
        fn build_hasher(&self) -> Self::Hasher {
            <ahash::RandomState as std::hash::BuildHasher>::build_hasher(&self.0)
        }
    }

    pub type HashMap<K, V> = std::collections::HashMap<K, V, DeterministicState>;
    pub type HashSet<T> = std::collections::HashSet<T, DeterministicState>;
}

pub use sv_parser::{self, parse, lexer, preprocessor, diagnostics, ParseResult, ast};
pub use value::Value;
pub use elaborate::{elaborate_module, ElaboratedModule};

/// Magic bytes identifying a xezim compiled artifact.
/// Version byte: \x07 = \x06 + Value is_fill field (§5.7.1 unbased-unsized);
/// \x06 = \x05 + serialized source_files/src_file_of_module
/// (cache-hit file:line diagnostics); \x05 = \x04 + const-NBA and branch fusion opcodes; \x04 = \x03 encoding + fused load-select opcodes
/// (LoadSignalRange/LoadSignalBit) in cached bytecode; \x03 =
/// zstd-compressed varint bincode body (\x02 = uncompressed varint,
/// \x01 = uncompressed fixint).
pub const XEZIM_BYTECODE_MAGIC: &[u8; 8] = b"XEZIMBC\x08";

/// zstd compression level used for `.xez` artifacts. Level 3 is zstd's own
/// default — strong compression at high throughput. Empirically shrinks
/// the elaborated-bincode stream ~27×, which more than pays for the
/// compute via reduced disk I/O.
const XEZIM_ZSTD_LEVEL: i32 = 3;

/// Bincode configuration for xezim compiled artifacts. Variable-int encoding
/// shrinks length tags, enum discriminants, and small integers; the wire
/// format is incompatible with the top-level `bincode::serialize` defaults
/// (which use fixed 8-byte ints), so this is the single source of truth used
/// by both writer and reader.
pub fn xez_bincode_options() -> impl bincode::Options + Copy {
    use bincode::Options;
    bincode::DefaultOptions::new()
        .with_varint_encoding()
        .with_little_endian()
}

fn artifact_version_error(file_magic: &[u8; 8]) -> Option<String> {
    if &file_magic[..7] == &XEZIM_BYTECODE_MAGIC[..7] && file_magic[7] != XEZIM_BYTECODE_MAGIC[7] {
        Some(format!(
            "incompatible xezim artifact version (file v{}, expected v{}); recompile with current xezim",
            file_magic[7], XEZIM_BYTECODE_MAGIC[7]
        ))
    } else {
        None
    }
}

/// Serialize a compiled ElaboratedModule to a file. Streams bincode through
/// a zstd encoder into the file; never holds the full serialized blob in
/// memory, and writes ~27× less to disk than the raw bincode stream.
pub fn write_compiled(elab: &elaborate::ElaboratedModule, path: &str) -> Result<(), String> {
    use bincode::Options;
    use std::io::Write;
    let f = std::fs::File::create(path).map_err(|e| format!("create '{}': {}", path, e))?;
    let mut w = std::io::BufWriter::with_capacity(1 << 20, f);
    w.write_all(XEZIM_BYTECODE_MAGIC).map_err(|e| format!("write '{}': {}", path, e))?;
    let mut enc = zstd::stream::Encoder::new(w, XEZIM_ZSTD_LEVEL)
        .map_err(|e| format!("zstd init: {}", e))?;
    xez_bincode_options()
        .serialize_into(&mut enc, elab)
        .map_err(|e| format!("serialize: {}", e))?;
    let mut w = enc.finish().map_err(|e| format!("zstd finish: {}", e))?;
    w.flush().map_err(|e| format!("flush '{}': {}", path, e))
}

/// Read a compiled artifact from a file. Returns Ok(Some(elab)) if the file is
/// a valid artifact, Ok(None) if it lacks the magic header, Err on I/O,
/// version-mismatch, or deserialization failure.
pub fn read_compiled(path: &str) -> Result<Option<elaborate::ElaboratedModule>, String> {
    use bincode::Options;
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| format!("read '{}': {}", path, e))?;
    let mut r = std::io::BufReader::with_capacity(1 << 20, f);
    let mut magic = [0u8; 8];
    if r.read_exact(&mut magic).is_err() {
        return Ok(None);
    }
    if &magic != XEZIM_BYTECODE_MAGIC {
        if let Some(err) = artifact_version_error(&magic) {
            return Err(err);
        }
        return Ok(None);
    }
    let dec = zstd::stream::Decoder::new(r).map_err(|e| format!("zstd init: {}", e))?;
    let elab = xez_bincode_options()
        .deserialize_from(dec)
        .map_err(|e| format!("deserialize: {}", e))?;
    Ok(Some(elab))
}

/// Like `read_compiled` but reads from an in-memory slice (e.g. an embedded
/// `include_bytes!()` payload in a binary produced by `--emit-native`).
pub fn read_compiled_bytes(bytes: &[u8]) -> Result<elaborate::ElaboratedModule, String> {
    use bincode::Options;
    if bytes.len() < 8 {
        return Err("xezim artifact: payload shorter than magic header".to_string());
    }
    let (magic, body) = bytes.split_at(8);
    if magic != XEZIM_BYTECODE_MAGIC {
        let mut m = [0u8; 8];
        m.copy_from_slice(magic);
        if let Some(err) = artifact_version_error(&m) {
            return Err(err);
        }
        return Err("xezim artifact: missing magic header".to_string());
    }
    let dec = zstd::stream::Decoder::new(body).map_err(|e| format!("zstd init: {}", e))?;
    xez_bincode_options()
        .deserialize_from(dec)
        .map_err(|e| format!("deserialize: {}", e))
}

use std::rc::Rc;

/// Implementation-defined `--module-timescale` command-line extension.
/// `global` applies to every module with no explicit source-level timescale;
/// `named` applies to the listed modules likewise. Exponents are powers of ten
/// in seconds (e.g. `1ns` = -9). Never overrides an explicit timescale.
#[derive(Clone, Default)]
pub struct ModuleTimescaleCli {
    pub global: Option<(i32, i32)>,
    pub named: std::collections::HashMap<String, (i32, i32)>,
}

/// Library-search configuration from the CLI (`-v <file>`, `-y <dir>`,
/// `+libext+<ext>`),
/// consumed by `resolve_library_modules`. Commercial semantics: a `-v` file's
/// definitions are adopted only to satisfy unresolved instantiations (never
/// top candidates); `+libext+` REPLACES the default `-y` extension list.
#[derive(Default, Clone)]
pub struct LibraryCli {
    pub lib_files: Vec<String>,
    pub lib_dirs: Vec<String>,
    /// `None` = default extensions (v, sv, V); `Some(list)` = exactly `list`.
    pub lib_exts: Option<Vec<String>>,
    /// Emit detailed parse and adoption diagnostics for explicit `-v` files.
    pub primitive_verbose: bool,
}

static LIBRARY_CLI: std::sync::OnceLock<std::sync::Mutex<LibraryCli>> = std::sync::OnceLock::new();

/// Whether `--primitive-verbose` is active — read by the simulator's UDP
/// lowering to print per-terminal resolution detail.
pub fn primitive_verbose() -> bool {
    library_cli_cell().lock().map(|g| g.primitive_verbose).unwrap_or(false)
}

fn library_cli_cell() -> &'static std::sync::Mutex<LibraryCli> {
    LIBRARY_CLI.get_or_init(|| std::sync::Mutex::new(LibraryCli::default()))
}

/// `-xenowarn`: suppress the §6.10 "implicit 1-bit net created" warnings.
/// Gate-level customer designs with thousands of vendor-cell pins can emit
/// thousands of these; the flag silences the WARNING while keeping the
/// implicit-net behavior itself (and the `default_nettype none error).
static IMPLICIT_NET_WARN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

pub fn set_implicit_net_warn(on: bool) {
    IMPLICIT_NET_WARN.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn implicit_net_warn() -> bool {
    IMPLICIT_NET_WARN.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_library_cli(cfg: LibraryCli) {
    *library_cli_cell().lock().unwrap() = cfg;
}

static MODULE_TIMESCALE_CLI: std::sync::OnceLock<std::sync::Mutex<ModuleTimescaleCli>> =
    std::sync::OnceLock::new();

fn module_timescale_cli_cell() -> &'static std::sync::Mutex<ModuleTimescaleCli> {
    MODULE_TIMESCALE_CLI.get_or_init(|| std::sync::Mutex::new(ModuleTimescaleCli::default()))
}

/// Install the parsed `--module-timescale` configuration before elaboration.
pub fn set_module_timescale_cli(cli: ModuleTimescaleCli) {
    if let Ok(mut g) = module_timescale_cli_cell().lock() {
        *g = cli;
    }
}

fn module_timescale_cli() -> ModuleTimescaleCli {
    module_timescale_cli_cell().lock().map(|g| g.clone()).unwrap_or_default()
}

#[derive(Debug, Clone)]
pub enum SourceDefinition {
    Module(Rc<ast::module::ModuleDeclaration>),
    Interface(Rc<ast::module::InterfaceDeclaration>),
    Program(Rc<ast::module::ProgramDeclaration>),
    Class(Rc<ast::decl::ClassDeclaration>),
    Package(Rc<ast::module::PackageDeclaration>),
    Typedef(Rc<ast::decl::TypedefDeclaration>),
    /// IEEE 1800-2017 §29 User-Defined Primitive.
    Udp(Rc<ast::decl::UdpDecl>),
}

impl SourceDefinition {
    pub fn name(&self) -> String {
        match self {
            SourceDefinition::Module(m) => m.name.name.clone(),
            SourceDefinition::Interface(i) => i.name.name.clone(),
            SourceDefinition::Program(p) => p.name.name.clone(),
            SourceDefinition::Class(c) => c.name.name.clone(),
            SourceDefinition::Package(p) => p.name.name.clone(),
            SourceDefinition::Typedef(t) => t.name.name.clone(),
            SourceDefinition::Udp(u) => u.name.name.clone(),
        }
    }

    pub fn items(&self) -> &[ast::decl::ModuleItem] {
        match self {
            SourceDefinition::Module(m) => &m.items,
            SourceDefinition::Interface(i) => &i.items,
            SourceDefinition::Program(p) => &p.items,
            SourceDefinition::Class(_) | SourceDefinition::Package(_)
            | SourceDefinition::Typedef(_) | SourceDefinition::Udp(_) => &[],
        }
    }
}

/// Tokenize a source string.
pub fn tokenize_file(source: &str, _path: Option<&std::path::Path>) -> Vec<lexer::Token> {
    lexer::Lexer::new(source).tokenize()
}

/// Parse a source string into an AST.
pub fn parse_str(source: &str) -> Result<ParseResult, Vec<diagnostics::Diagnostic>> {
    let result = sv_parser::parse(source);
    if !result.errors.is_empty() {
        Err(result.errors)
    } else {
        Ok(result)
    }
}

pub fn parse_and_elaborate_multi(
    sources: &[String],
    top_module_name: Option<&str>,
    include_dirs: &[String],
    source_files: &[String],
    defines: &[(String, Option<String>)],
) -> Result<(crate::hasher::HashMap<String, SourceDefinition>, elaborate::ElaboratedModule), String> {
    let mut all_descriptions = Vec::new();
    // Preprocessed text of each source, kept in parse order. Every AST
    // `Span` is a byte offset into ITS file's preprocessed text, so these
    // are what runtime diagnostics need to turn a span into `file:line`
    // (see `ElaboratedModule::source_texts`).
    let mut preprocessed_texts: Vec<String> = Vec::with_capacity(sources.len());
    // Which file defined each module/interface/program, by name. Captured
    // HERE — the only point where a description's originating file is still
    // known — and handed to runtime diagnostics via
    // `ElaboratedModule::src_file_of_module` (see that field's doc).
    let mut src_file_of_module: crate::hasher::HashMap<String, u32> =
        crate::hasher::HashMap::default();
    let mut pp = preprocessor::Preprocessor::new();
    for dir in include_dirs { pp.add_include_dir(std::path::PathBuf::from(dir)); }
    for (name, val) in defines {
        pp.define(name.clone(), preprocessor::MacroDef {
            name: name.clone(), params: None,
            body: val.clone().unwrap_or_default(),
        });
    }

    for (i, source) in sources.iter().enumerate() {
        let source_path = source_files.get(i).map(|p| std::path::PathBuf::from(p));
        // Mark a new compilation file so a `timescale that stuck across from a
        // prior file is treated as inherited (overridable by --module-timescale)
        // rather than declared here.
        pp.begin_top_level_file();
        let preprocessed = pp.preprocess_file(source, source_path.as_deref());

        let tokens = lexer::Lexer::new(&preprocessed).tokenize();
        let mut parser = sv_parser::parse::Parser::new(tokens);
        let source_ast = parser.parse_source_text();
        let diags = parser.diagnostics().to_vec();

        if diags.iter().any(|d| d.severity == diagnostics::Severity::Error) {
            let errs: Vec<_> = diags.iter()
                .filter(|d| d.severity == diagnostics::Severity::Error)
                .map(|d| d.to_string()).collect();
            return Err(format!("Parse errors in source {}:\n{}", i, errs.join("\n")));
        }
        // Second, AST-level strict pass (runs alongside the permissive parser;
        // gated by --strict, on by default). Rejects LRM violations the main
        // parser accepts. See sv_parser::strict_check.
        let strict_viol = sv_parser::strict_check::strict_violations(&source_ast.descriptions);
        if !strict_viol.is_empty() {
            return Err(format!("Strict check failed in source {}:\n{}", i, strict_viol.join("\n")));
        }
        for d in &source_ast.descriptions {
            let name = match d {
                ast::Description::Module(m) => Some(&m.name.name),
                ast::Description::Interface(iface) => Some(&iface.name.name),
                ast::Description::Program(p) => Some(&p.name.name),
                ast::Description::PackageItem(ast::decl::PackageItem::Checker(c)) => {
                    Some(&c.name.name)
                }
                _ => None,
            };
            if let Some(name) = name {
                src_file_of_module.entry(name.clone()).or_insert(i as u32);
            }
        }
        all_descriptions.extend(source_ast.descriptions);
        preprocessed_texts.push(preprocessed);
    }

    let lib_defines = pp.snapshot_defines();
    let module_timescales = pp.module_timescales.clone();
    let module_ts_own_file = pp.module_ts_own_file.clone();
    let (defs, mut elab) =
        parse_and_elaborate(all_descriptions, top_module_name, include_dirs, &lib_defines, &module_timescales, &module_ts_own_file)?;
    elab.source_texts = preprocessed_texts;
    elab.source_files = source_files.to_vec();
    elab.src_file_of_module = src_file_of_module;
    Ok((defs, elab))
}

fn parse_and_elaborate(
    all_descriptions: Vec<ast::Description>,
    top_module_name: Option<&str>,
    include_dirs: &[String],
    lib_defines: &std::collections::HashMap<String, preprocessor::MacroDef>,
    module_timescales: &std::collections::HashMap<String, (f64, f64)>,
    module_ts_own_file: &std::collections::HashSet<String>,
) -> Result<(crate::hasher::HashMap<String, SourceDefinition>, elaborate::ElaboratedModule), String> {
    // Effective per-module timescale, unifying `\`timescale` directives (from
    // the preprocessor) with in-body `timeunit`/`timeprecision` declarations
    // (§3.14.2). A `timeunit` decl was previously ignored here, so its module's
    // delays were never scaled — `#5` in a `timeunit 1us` module ran as 5 ns.
    // Precedence, highest first (implementation-defined --module-timescale
    // extension): local timeunit/timeprecision decl > active `\`timescale`
    // directive > named --module-timescale > global --module-timescale >
    // 1 ns / 1 ns default. The command-line forms never override an explicit
    // source-level timescale (a local decl OR an active directive).
    let cli = module_timescale_cli();
    let mut eff_ts: std::collections::HashMap<String, (f64, f64)> = std::collections::HashMap::new();
    let mut named_matched: std::collections::HashSet<String> = std::collections::HashSet::new();
    // §3.14.2.2 — track modules that carry NO `timescale (no source-level
    // decl, no preceding directive, no CLI override) so a mixed design (some
    // modules timed, some not) can be warned about after the pass.
    let mut any_explicit_ts = false;
    let mut modules_without_ts: Vec<String> = Vec::new();
    for desc in &all_descriptions {
        if let ast::Description::Module(m) = desc {
            let name = &m.name.name;
            // Explicit source-level declarations.
            let mut local_u: Option<i32> = None;
            let mut local_p: Option<i32> = None;
            for it in &m.items {
                if let ast::decl::ModuleItem::TimeunitsDecl(td) = it {
                    if let Some(u) = &td.unit {
                        local_u = Some(elaborate::time_literal_to_exp(u));
                    }
                    if let Some(p) = &td.precision {
                        local_p = Some(elaborate::time_literal_to_exp(p));
                    }
                }
            }
            let directive = module_timescales
                .get(name)
                .map(|&(u, p)| (elaborate::secs_to_exp(u), elaborate::secs_to_exp(p)));
            // A `\`timescale` that STUCK across from a prior file (inherited) is
            // NOT the module's own source-level timescale. `--module-timescale`
            // may override such an inheritance; only a local `timeunit` decl or a
            // directive in the module's OWN file is truly "explicit source-level"
            // and wins over the CLI.
            let directive_own_file = directive.is_some() && module_ts_own_file.contains(name);
            let own_explicit = local_u.is_some() || local_p.is_some() || directive_own_file;
            let named = cli.named.get(name).copied();
            let cli_ts = named.or(cli.global);
            if named.is_some() {
                named_matched.insert(name.clone());
            }
            if own_explicit {
                any_explicit_ts = true;
            } else if directive.is_none() && cli_ts.is_none() {
                // Genuinely no timescale anywhere (own or inherited) and no CLI.
                modules_without_ts.push(name.clone());
            }

            let eff_exp: Option<(i32, i32)> = if own_explicit {
                // A local decl overrides the (own-file) directive field by field;
                // a missing field falls back to the directive, then to 1 ns.
                let (du, dp) = directive.unwrap_or((-9, -9));
                let u = local_u.unwrap_or(du);
                let p = local_p.unwrap_or(dp);
                if named.is_some() {
                    eprintln!(
                        "[warn] --module-timescale for module '{}' ignored; it has an explicit source-level timescale",
                        name
                    );
                }
                Some((u, p))
            } else if let Some(ts) = cli_ts {
                // --module-timescale supplies the timescale, OVERRIDING any
                // directive merely inherited (sticky) from a prior file.
                Some(ts)
            } else {
                // No CLI: keep a cross-file-inherited directive (single-compilation-
                // unit sticky behavior) when present; else no timescale.
                directive
            };
            if let Some((u, p)) = eff_exp {
                eff_ts.insert(name.clone(), (elaborate::exp_to_secs(u), elaborate::exp_to_secs(p)));
            }
        }
    }
    // §3.14.2.2 — a design that MIXES timed and untimed modules is a common
    // source of surprise (the untimed module falls back to the default unit).
    // Warn once per untimed module, but only in the mixed case (a fully
    // untimed design has a uniform default and needs no warning).
    if any_explicit_ts {
        for name in &modules_without_ts {
            eprintln!(
                "[warn] module '{}' has no timescale directive; defaulting its reported timescale to 1s/1s",
                name
            );
        }
    }
    // §10: warn on a named assignment that matched no module definition.
    for name in cli.named.keys() {
        if !named_matched.contains(name) {
            eprintln!("[warn] --module-timescale did not match module '{}'", name);
        }
    }

    // Global simulation tick = the finest precision across the design
    // (default 1 ns). All module delays are then pre-scaled to this unit.
    let tick_s = eff_ts.values().map(|&(_, p)| p).fold(1e-9_f64, f64::min);
    let mut module_timescale_exp: crate::hasher::HashMap<String, (i32, i32)> =
        crate::hasher::HashMap::default();
    for (n, &(u, p)) in &eff_ts {
        module_timescale_exp.insert(n.clone(), (elaborate::secs_to_exp(u), elaborate::secs_to_exp(p)));
    }
    let mut definitions: crate::hasher::HashMap<String, SourceDefinition> = crate::hasher::HashMap::default();
    let mut top_module = None;
    let mut top_level_imports = Vec::new();
    let mut top_level_lets = Vec::new();
    let mut top_level_functions: Vec<ast::decl::FunctionDeclaration> = Vec::new();
    let mut top_level_tasks: Vec<ast::decl::TaskDeclaration> = Vec::new();
    let mut top_level_nettypes: Vec<ast::decl::NettypeDeclaration> = Vec::new();
    let mut top_level_params: Vec<ast::decl::ParameterDeclaration> = Vec::new();
    let mut top_level_vars: Vec<ast::decl::DataDeclaration> = Vec::new();
    let mut top_level_binds: Vec<ast::decl::BindDirective> = Vec::new();
    // §18.5.1 $unit-scope out-of-class constraint definitions (class, name).
    let mut top_level_ooc_constraints: Vec<(String, String, Vec<ast::decl::ConstraintItem>)> =
        Vec::new();
    for desc in all_descriptions {
        match desc {
            ast::Description::Module(mut m) => {
                let name = m.name.name.clone();
                if definitions.contains_key(&name) {
                    return Err(format!(
                        "Duplicate module definition '{}' (IEEE 1800-2017 §3.3)",
                        name
                    ));
                }
                // Pre-scale this module's delays from its own timeunit to the
                // global tick (no-op when both are 1 ns).
                // Every module's delays are pre-scaled to the global tick, even
                // those with no explicit or CLI timescale — the simulator
                // consumes tick-denominated delays. A module with no effective
                // timescale uses the tick unit, making the rewrite a numeric
                // no-op but still converting the delay form.
                let unit_s = eff_ts.get(&name).map(|&(u, _)| u).unwrap_or(tick_s);
                elaborate::rewrite_module_delays_pub(&mut m.items, unit_s, tick_s);
                top_module = Some(name.clone());
                definitions.insert(name, SourceDefinition::Module(Rc::new(m)));
            }
            ast::Description::Interface(i) => {
                let name = i.name.name.clone();
                definitions.insert(name, SourceDefinition::Interface(Rc::new(i)));
            }
            ast::Description::Program(p) => {
                let name = p.name.name.clone();
                top_module = Some(name.clone());
                definitions.insert(name, SourceDefinition::Program(Rc::new(p)));
            }
            ast::Description::Class(c) => {
                let name = c.name.name.clone();
                definitions.insert(name, SourceDefinition::Class(Rc::new(c)));
            }
            ast::Description::Package(p) => {
                let name = p.name.name.clone();
                definitions.insert(name, SourceDefinition::Package(Rc::new(p)));
            }
            ast::Description::TypedefDecl(t) => {
                let name = t.name.name.clone();
                // §6.18: a bare forward typedef (`typedef name;`) must not
                // displace a real definition of the same name — `typedef_test_0`
                // restates the forward name after the full `typedef int name;`.
                // Forward → insert only if absent; real → always (replaces a
                // prior forward placeholder).
                if t.forward {
                    definitions.entry(name).or_insert_with(|| SourceDefinition::Typedef(Rc::new(t)));
                } else {
                    definitions.insert(name, SourceDefinition::Typedef(Rc::new(t)));
                }
            }
            ast::Description::ImportDecl(id) => {
                top_level_imports.push(id);
            }
            ast::Description::PackageItem(ast::decl::PackageItem::Checker(c)) => {
                let m = ast::module::ModuleDeclaration {
                    attrs: Vec::new(),
                    kind: ast::module::ModuleKind::Module,
                    lifetime: None,
                    name: c.name,
                    params: Vec::new(),
                    ports: c.ports,
                    items: c.items,
                    endlabel: c.endlabel,
                    span: c.span,
                };
                let name = m.name.name.clone();
                definitions.insert(name, SourceDefinition::Module(Rc::new(m)));
            }
            ast::Description::PackageItem(ast::decl::PackageItem::Let(l)) => {
                top_level_lets.push(l);
            }
            ast::Description::PackageItem(ast::decl::PackageItem::Function(f)) => {
                top_level_functions.push(f);
            }
            ast::Description::PackageItem(ast::decl::PackageItem::Task(t)) => {
                top_level_tasks.push(t);
            }
            ast::Description::PackageItem(ast::decl::PackageItem::Nettype(n)) => {
                top_level_nettypes.push(n);
            }
            ast::Description::PackageItem(ast::decl::PackageItem::Parameter(p)) => {
                top_level_params.push(p);
            }
            ast::Description::PackageItem(ast::decl::PackageItem::Data(d)) => {
                top_level_vars.push(d);
            }
            ast::Description::Bind(b) => {
                top_level_binds.push(b);
            }
            ast::Description::OutOfClassConstraint { class_name, constraint_name, items } => {
                top_level_ooc_constraints.push((class_name, constraint_name, items));
            }
            ast::Description::Udp(u) => {
                // IEEE 1800-2017 §29: register the UDP in the definition map so
                // instantiations resolve to it (elaboration lowers each
                // instance into a runtime truth-table evaluator).
                let name = u.name.name.clone();
                // Mirror the historical behavior where a UDP parsed as an empty
                // `Description::Module` advanced the source-order `top_module`
                // cursor. A UDP is never a real hierarchy root, but keeping this
                // cursor identical preserves auto-top-detection: a following
                // heuristic re-selects a proper module/program candidate, and
                // leaving the cursor on a trailing UDP (an instantiated,
                // non-candidate name) correctly forces that heuristic instead of
                // pinning a trivial trailing program/package.
                top_module = Some(name.clone());
                definitions.insert(name, SourceDefinition::Udp(Rc::new(u)));
            }
            _ => {}
        }
    }

    // §23.11: a `bind` written as a module item (not at compilation-unit
    // scope) is applied identically. Lift every `ModuleItem::Bind` out of the
    // module bodies (replacing it with `Null` so it is not re-processed as a
    // real instantiation) and fold it into `top_level_binds`.
    let mut inmodule_binds: Vec<ast::decl::BindDirective> = Vec::new();
    for def in definitions.values_mut() {
        if let SourceDefinition::Module(m) = def {
            if !m.items.iter().any(|it| matches!(it, ast::decl::ModuleItem::Bind(_))) {
                continue;
            }
            let m = Rc::make_mut(m);
            for it in m.items.iter_mut() {
                if let ast::decl::ModuleItem::Bind(b) = it {
                    inmodule_binds.push(b.clone());
                    *it = ast::decl::ModuleItem::Null;
                }
            }
        }
    }
    top_level_binds.extend(inmodule_binds);

    // IEEE 1800-2023 §23.11: apply each `bind` by appending the bound
    // instantiation to its target module's items. This runs before
    // top_level_functions/tasks/nettypes injection so a bound monitor module
    // sees the same top-level helpers as native modules.
    for b in &top_level_binds {
        let tname = b.target_module.name.clone();
        let Some(def) = definitions.get_mut(&tname) else { continue };
        if let SourceDefinition::Module(m) = def {
            let m = Rc::make_mut(m);
            m.items.push(ast::decl::ModuleItem::ModuleInstantiation(b.instantiation.clone()));
        }
    }
    if !top_level_functions.is_empty() || !top_level_tasks.is_empty()
        || !top_level_nettypes.is_empty() || !top_level_params.is_empty()
        || !top_level_vars.is_empty() {
        for def in definitions.values_mut() {
            if let SourceDefinition::Module(m) = def {
                let m = Rc::make_mut(m);
                for f in top_level_functions.iter().rev() {
                    m.items.insert(0, ast::decl::ModuleItem::FunctionDeclaration(f.clone()));
                }
                for t in top_level_tasks.iter().rev() {
                    m.items.insert(0, ast::decl::ModuleItem::TaskDeclaration(t.clone()));
                }
                for n in top_level_nettypes.iter().rev() {
                    m.items.insert(0, ast::decl::ModuleItem::NettypeDeclaration(n.clone()));
                }
                // $unit-scope parameters become body localparams (constants):
                // visible inside the module, not part of its override interface.
                // A module-local parameter/localparam of the same name SHADOWS
                // the $unit one (LRM §3.12.1 name resolution) — skip injecting
                // any $unit param the module already declares, else the two
                // collide as a "Duplicate declaration".
                let local_param_names: std::collections::HashSet<String> = m.items.iter()
                    .filter_map(|it| match it {
                        ast::decl::ModuleItem::ParameterDeclaration(pd)
                        | ast::decl::ModuleItem::LocalparamDeclaration(pd) => Some(pd),
                        _ => None,
                    })
                    .flat_map(|pd| match &pd.kind {
                        ast::decl::ParameterKind::Data { assignments, .. } =>
                            assignments.iter().map(|a| a.name.name.clone()).collect::<Vec<_>>(),
                        _ => Vec::new(),
                    })
                    .collect();
                for p in top_level_params.iter().rev() {
                    let shadowed = match &p.kind {
                        ast::decl::ParameterKind::Data { assignments, .. } =>
                            assignments.iter().all(|a| local_param_names.contains(&a.name.name)),
                        _ => false,
                    };
                    if shadowed { continue; }
                    m.items.insert(0, ast::decl::ModuleItem::LocalparamDeclaration(p.clone()));
                }
                // $unit-scope variables (`string label = "X";`) become module
                // signals so references — including from class methods
                // validated against this module — resolve.
                for d in top_level_vars.iter().rev() {
                    m.items.insert(0, ast::decl::ModuleItem::DataDeclaration(d.clone()));
                }
            }
        }
    }
    // Capture the definitions that came from the explicitly-provided source
    // files BEFORE pulling in `-y` / `-v` library modules. Library
    // modules satisfy instantiations but must NEVER be candidates for the
    // implicit top: otherwise compiling a self-contained file (e.g. a lone
    // `class`) that shares an include dir with sibling testbenches would let
    // one of those testbenches (`module tb; initial run_test(); …`) get picked
    // as the top and run. An include dir is a search path, not a compile list.
    let explicit_def_names: std::collections::HashSet<String> =
        definitions.keys().cloned().collect();
    let lib_cli = library_cli_cell().lock().unwrap().clone();
    if !lib_cli.lib_dirs.is_empty() || !lib_cli.lib_files.is_empty() {
        resolve_library_modules(&mut definitions, include_dirs, lib_defines, &lib_cli)?;

        // A `-v`/`-y` library module is adopted AFTER the primary-source delay
        // rewrite (above), so it never received a timescale — its `#delay`s
        // stayed raw tick-unit values while the same module compiled as a
        // primary source would be scaled. Apply `--module-timescale` (named,
        // else global) to each newly-adopted library module so its delays scale
        // consistently. Library modules with no CLI timescale keep tick units
        // (their own `` `timescale `` directive is a separate, not-yet-captured
        // path).
        if cli.global.is_some() || !cli.named.is_empty() {
            let lib_names: Vec<String> = definitions
                .keys()
                .filter(|n| !explicit_def_names.contains(*n))
                .cloned()
                .collect();
            for name in lib_names {
                let ts = cli.named.get(&name).copied().or(cli.global);
                let Some((u, p)) = ts else { continue };
                let unit_s = elaborate::exp_to_secs(u);
                let _ = p; // precision folds into the global tick, already fixed
                if let Some(SourceDefinition::Module(rc)) = definitions.get_mut(&name) {
                    let m = Rc::make_mut(rc);
                    elaborate::rewrite_module_delays_pub(&mut m.items, unit_s, tick_s);
                    module_timescale_exp
                        .insert(name.clone(), (elaborate::secs_to_exp(unit_s), elaborate::secs_to_exp(elaborate::exp_to_secs(p))));
                }
            }
        }
    }

    let named_top_found = top_module_name.map_or(false, |n| definitions.contains_key(n));
    if let (Some(name), true) = (top_module_name, named_top_found) {
        top_module = Some(name.to_string());
    } else {
        // No top named, OR the named top wasn't found — auto-detect the
        // hierarchy root (a module instantiated by no other). This recovers
        // from a wrong `:top_module:` in a generated test (e.g. sv-tests'
        // veer-el2 specifies `veer-el2_wrapper`, but the module is
        // `el2_veer_wrapper`).
        if let Some(name) = top_module_name {
            eprintln!("[xezim][warning] top module '{}' not found; auto-detecting the design root", name);
        }
        let mut instantiated: std::collections::HashSet<String> = std::collections::HashSet::new();
        for m in definitions.values() { collect_instantiated_modules(m.items(), &mut instantiated); }
        let mut candidates: Vec<String> = definitions.keys()
            .filter(|n| !instantiated.contains(n.as_str())
                && explicit_def_names.contains(n.as_str())
                // A top-level (`$unit`-scope) typedef is never a hierarchy
                // root. Without this a file like `typedef enum {...} T;
                // module test; ...` wrongly picked `T`, which then fails
                // elaboration ("not a module or program"). Packages stay
                // eligible: a package-only design (e.g. uvm_pkg) legitimately
                // elaborates the package as the root.
                && !matches!(definitions.get(n.as_str()), Some(SourceDefinition::Typedef(_)))
                // §29: a UDP is never a hierarchy root — exclude it so a
                // trailing/unused primitive can't pin auto-top-detection.
                && !matches!(definitions.get(n.as_str()), Some(SourceDefinition::Udp(_))))
            .cloned().collect();
        // Sort to make top-module selection deterministic when more than one
        // module is uninstantiated. Without this, ahash's random seed picks
        // arbitrarily between, e.g., openc910's `tb` and `top` testbenches —
        // each iteration runs a different testbench's initial blocks, so the
        // sim either fires up clk/rst correctly or silently picks the
        // verilator variant whose forever-counter logic xezim doesn't model.
        candidates.sort();
        // If the source-order parse already picked a top that's a valid
        // candidate (uninstantiated by anything else), prefer it over the
        // candidate-based heuristic. Otherwise fall through to the heuristic
        // and rely on `candidates.sort()` for determinism.
        let parse_pick_valid = top_module.as_ref()
            .map_or(false, |n| candidates.iter().any(|c| c == n));
        if parse_pick_valid {
            // Keep top_module as-is — deterministic via source order.
        } else if candidates.len() == 1 {
            top_module = Some(candidates[0].clone());
        } else if candidates.len() > 1 {
            for c in &candidates {
                if definitions.get(c).unwrap().items().iter().any(|item| matches!(item, ast::decl::ModuleItem::InitialConstruct(_))) {
                    top_module = Some(c.clone()); break;
                }
            }
        }
        // No candidate carried an `initial` (e.g. a file of several `class`
        // declarations with no module — common in §18 constrained-random
        // tests). Rather than failing with "No module found", fall back to the
        // first candidate so the design still elaborates: a single-class file
        // already behaved this way, and a multi-class file should too.
        if top_module.is_none() && !candidates.is_empty() {
            top_module = Some(candidates[0].clone());
        }
    }

    let top_name = top_module.ok_or("No module found")?;
    let top_def = definitions.get(&top_name).ok_or_else(|| format!("Module '{}' not found", top_name))?;
    let params = crate::hasher::HashMap::default();

    let def_refs: crate::hasher::HashMap<String, elaborate::Definition> =
        definitions.iter().filter_map(|(k, v)| {
            let def = match v {
                SourceDefinition::Module(m) => elaborate::Definition::Module(&**m),
                SourceDefinition::Interface(i) => elaborate::Definition::Interface(&**i),
                SourceDefinition::Program(p) => elaborate::Definition::Program(&**p),
                SourceDefinition::Class(c) => elaborate::Definition::Class(&**c),
                SourceDefinition::Package(p) => elaborate::Definition::Package(&**p),
                SourceDefinition::Typedef(t) => elaborate::Definition::Typedef(&**t),
                SourceDefinition::Udp(u) => elaborate::Definition::Udp(&**u),
            };
            Some((k.clone(), def))
        }).collect();

    let elab_def = match top_def {
        SourceDefinition::Module(m) => elaborate::Definition::Module(&**m),
        SourceDefinition::Interface(i) => elaborate::Definition::Interface(&**i),
        SourceDefinition::Program(p) => elaborate::Definition::Program(&**p),
        SourceDefinition::Class(c) => elaborate::Definition::Class(&**c),
        SourceDefinition::Package(p) => elaborate::Definition::Package(&**p),
        _ => return Err(format!("Top-level element '{}' is not a module or program", top_name)),
    };
    let mut elab = elaborate::elaborate_module_with_defs(
        elab_def,
        &params,
        Some(&def_refs),
        &top_level_imports,
        &top_level_lets,
        &top_level_ooc_constraints,
    )?;
    elab.tick_s = tick_s;
    elab.module_timescale_exp = module_timescale_exp;
    // The top module's own unit/precision drives the default $time scaling and
    // $printtimescale when no per-scope entry is found.
    if let Some(&(u, p)) = elab.module_timescale_exp.get(&elab.name) {
        elab.timeunit_exp = u;
        elab.timeprecision_exp = p;
    }

    elaborate::inline_instantiations(&mut elab, &def_refs)?;
    // §28.8: bidirectional switches need every terminal's drivers in hand.
    elaborate::resolve_bidirectional_switches(&mut elab);
    // §6.6.1: a net with several continuous drivers resolves them all.
    elaborate::resolve_multi_driver_nets(&mut elab);
    // Link `function ClassName::m(); ...` out-of-class bodies into their
    // classes — must run after inline_instantiations repopulates classes.
    elaborate::link_extern_methods(&mut elab, &def_refs);
    if std::env::var("XEZIM_ELAB_STATS").is_ok() {
        eprintln!("[elab-stats] always_blocks={} initial_blocks={} cont_assigns={} pending_always={} pending_initial={} pending_cont_assign={} signals={} parameters={} arrays={} arrays_2d={} arrays_nd={} packed_struct_fields={}",
            elab.always_blocks.len(),
            elab.initial_blocks.len(),
            elab.continuous_assigns.len(),
            elab.pending_always.len(),
            elab.pending_initial.len(),
            elab.pending_cont_assign.len(),
            elab.signals.len(),
            elab.parameters.len(),
            elab.arrays.len(),
            elab.arrays_2d.len(),
            elab.arrays_nd.len(),
            elab.packed_struct_fields.len(),
        );
        // Bytewise breakdown via bincode serialize. Approximation only —
        // bincode is more compact than in-memory layout (no Vec capacity
        // slack, no padding, no String header) but the per-section relative
        // sizes correctly identify hot spots. On c910 hello this revealed
        // continuous_assigns at 394 MB, always_blocks at 193 MB, signals at
        // 173 MB — the three to target for memory work.
        use bincode::Options;
        let opts = xez_bincode_options();
        let try_size = |label: &str, bytes: Result<Vec<u8>, _>| match bytes {
            Ok(b) => eprintln!("[elab-bytes-bincode] {}: {:>10} bytes", label, b.len()),
            Err(_) => eprintln!("[elab-bytes-bincode] {}: <serialize failed>", label),
        };
        try_size("always_blocks    ", opts.serialize(&elab.always_blocks));
        try_size("initial_blocks   ", opts.serialize(&elab.initial_blocks));
        try_size("continuous_assigns", opts.serialize(&elab.continuous_assigns));
        try_size("signals          ", opts.serialize(&elab.signals));
        try_size("parameters       ", opts.serialize(&elab.parameters));
        try_size("arrays           ", opts.serialize(&elab.arrays));
        try_size("arrays_2d        ", opts.serialize(&elab.arrays_2d));
        try_size("arrays_nd        ", opts.serialize(&elab.arrays_nd));
        try_size("functions        ", opts.serialize(&elab.functions));
        try_size("tasks            ", opts.serialize(&elab.tasks));
        try_size("typedefs         ", opts.serialize(&elab.typedefs));
        try_size("typedef_types    ", opts.serialize(&elab.typedef_types));
        try_size("classes          ", opts.serialize(&elab.classes));
        try_size("specify_delays   ", opts.serialize(&elab.specify_delays));
    }
    Ok((definitions, elab))
}

fn collect_instantiated_modules(items: &[ast::decl::ModuleItem], set: &mut std::collections::HashSet<String>) {
    for item in items {
        match item {
            ast::decl::ModuleItem::ModuleInstantiation(mi) => { set.insert(mi.module_name.name.clone()); }
            ast::decl::ModuleItem::GenerateIf(gi) => {
                for (_cond, items) in &gi.branches { collect_instantiated_modules(items, set); }
            }
            ast::decl::ModuleItem::GenerateFor(gf) => collect_instantiated_modules(&gf.items, set),
            // A stdcell netlist routinely instantiates cells inside generate
            // regions, named generate blocks, and generate-case arms — these
            // were invisible to the library resolver, so `-v`/`-y` never
            // adopted the cell and elaboration failed with
            // "Module 'X' instantiated but not found".
            ast::decl::ModuleItem::GenerateRegion(gr) => {
                collect_instantiated_modules(&gr.items, set)
            }
            ast::decl::ModuleItem::GenerateCase(gc) => {
                for arm in &gc.arms { collect_instantiated_modules(&arm.items, set); }
            }
            _ => {}
        }
    }
}

fn resolve_library_modules(
    definitions: &mut crate::hasher::HashMap<String, SourceDefinition>,
    include_dirs: &[String],
    lib_defines: &std::collections::HashMap<String, preprocessor::MacroDef>,
    lib_cli: &LibraryCli,
) -> Result<(), String> {
    fn collect_sv_files(
        dir: &std::path::Path,
        exts: &[String],
        out: &mut Vec<std::path::PathBuf>,
    ) -> Result<(), String> {
        let entries = std::fs::read_dir(dir)
            .map_err(|e| format!("read_dir '{}': {}", dir.display(), e))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("read_dir '{}': {}", dir.display(), e))?;
            let path = entry.path();
            if path.is_dir() {
                collect_sv_files(&path, exts, out)?;
                continue;
            }
            let Some(ext) = path.extension().and_then(|s| s.to_str()) else { continue };
            if exts.iter().any(|e| e == ext) {
                out.push(path);
            }
        }
        Ok(())
    }

    // `+libext+<ext>` REPLACES the default extension list (commercial
    // semantics); without it, `-y` searches .v/.sv/.V as before.
    let exts: Vec<String> = match &lib_cli.lib_exts {
        Some(list) => list.clone(),
        None => vec!["v".into(), "sv".into(), "V".into()],
    };

    let mut files: Vec<(std::path::PathBuf, bool)> = Vec::new();
    for dir in &lib_cli.lib_dirs {
        let path = std::path::Path::new(dir);
        if path.is_dir() {
            let mut found = Vec::new();
            collect_sv_files(path, &exts, &mut found)?;
            files.extend(found.into_iter().map(|path| (path, false)));
        }
    }
    // `-v <file>`: an explicit library FILE (any extension). Indexed like a
    // `-y` hit — its modules are adopted only when needed, never tops.
    for f in &lib_cli.lib_files {
        let path = std::path::PathBuf::from(f);
        if !path.is_file() {
            return Err(format!("-v library file not found: {}", f));
        }
        files.push((path, true));
    }

    // Index every library file's module/interface/program definitions (and
    // non-forward typedefs) by name WITHOUT adopting them yet. §23.3.2: a
    // library directory supplies definitions only to satisfy *unresolved*
    // instantiations. Adopting everything poisons the primary design's global
    // scope — sv-tests points `-I` at ivltests/ (~1000 unrelated single-file
    // tests), whose modules carry internal typedefs/enums (e.g. an unrelated
    // `typedef word word_darray[];`) that then fail the §6.18 base-type check
    // in a test that never mentions them.
    let mut lib: crate::hasher::HashMap<String, SourceDefinition> = Default::default();
    let mut lib_typedefs: Vec<Rc<ast::decl::TypedefDeclaration>> = Vec::new();
    let mut scanned_paths: Vec<std::path::PathBuf> = Vec::new();
    let mut parse_issue_files: Vec<std::path::PathBuf> = Vec::new();
    let mut lib_origins: crate::hasher::HashMap<String, (std::path::PathBuf, bool, &'static str)> =
        Default::default();
    for (path, explicit_v) in files {
        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Warning: library file '{}' unreadable: {}", path.display(), e);
                continue;
            }
        };
        scanned_paths.push(path.clone());
        let mut pp = preprocessor::Preprocessor::new();
        for dir in include_dirs {
            pp.add_include_dir(std::path::PathBuf::from(dir));
        }
        for (name, def) in lib_defines {
            pp.define(name.clone(), def.clone());
        }
        let preprocessed = pp.preprocess_file(&source, Some(&path));
        let result = sv_parser::parse(&preprocessed);
        let line_col = |off: usize| -> (usize, usize) {
            let (mut line, mut col) = (1usize, 1usize);
            for (i, ch) in preprocessed.char_indices() {
                if i >= off {
                    break;
                }
                if ch == '\n' {
                    line += 1;
                    col = 1;
                } else {
                    col += 1;
                }
            }
            (line, col)
        };
        if explicit_v && lib_cli.primitive_verbose {
            let mut modules = 0usize;
            let mut primitives = 0usize;
            for desc in &result.source.descriptions {
                match desc {
                    ast::Description::Module(m) => {
                        modules += 1;
                        eprintln!(
                            "[primitive-verbose] parsed module '{}' from -v '{}'",
                            m.name.name,
                            path.display()
                        );
                    }
                    ast::Description::Udp(u) => {
                        primitives += 1;
                        eprintln!(
                            "[primitive-verbose] parsed UDP '{}' from -v '{}': ports={} rows={} sequential={} init={:?}",
                            u.name.name,
                            path.display(),
                            u.ports.len(),
                            u.rows.len(),
                            u.is_sequential,
                            u.init
                        );
                    }
                    ast::Description::Interface(i) => eprintln!(
                        "[primitive-verbose] parsed interface '{}' from -v '{}'",
                        i.name.name,
                        path.display()
                    ),
                    ast::Description::Program(p) => eprintln!(
                        "[primitive-verbose] parsed program '{}' from -v '{}'",
                        p.name.name,
                        path.display()
                    ),
                    _ => {}
                }
            }
            eprintln!(
                "[primitive-verbose] -v parse summary '{}': bytes={} descriptions={} modules={} primitives={} errors={} warnings={}",
                path.display(),
                source.len(),
                result.source.descriptions.len(),
                modules,
                primitives,
                result.errors.len(),
                result.warnings.len()
            );
            if !result.errors.is_empty() || !result.warnings.is_empty() {
                eprintln!(
                    "[primitive-verbose] detailed parser diagnostics for -v '{}':",
                    path.display()
                );
                for (severity, diagnostic) in result
                    .errors
                    .iter()
                    .map(|d| ("error", d))
                    .chain(result.warnings.iter().map(|d| ("warning", d)))
                    .take(16)
                {
                    let (line, col) = line_col(diagnostic.span.start);
                    let source_line = preprocessed.lines().nth(line.saturating_sub(1)).unwrap_or("");
                    eprintln!(
                        "  {}:{}:{}: {}: {}",
                        path.display(),
                        line,
                        col,
                        severity,
                        diagnostic.message
                    );
                    eprintln!("    {}", source_line);
                    eprintln!("    {}^", " ".repeat(col.saturating_sub(1)));
                }
            }
        }
        // A half-parsed library file silently loses every definition after the
        // point of failure — the classic "-v vendor.v then Module 'X' not
        // found". Surface it VCS-style: file:line:col per error (first three),
        // resolved against the preprocessed text the spans index (line numbers
        // can shift from the raw file where `include/macros expand).
        if !result.errors.is_empty() {
            eprintln!(
                "Warning: library file '{}': {} parse error(s) — definitions after the first error may be lost:",
                path.display(),
                result.errors.len()
            );
            for e in result.errors.iter().take(3) {
                let (line, col) = line_col(e.span.start);
                eprintln!("  {}:{}:{}: {}", path.display(), line, col, e.message);
            }
            if result.errors.len() > 3 {
                eprintln!("  ... and {} more", result.errors.len() - 3);
            }
            parse_issue_files.push(path.clone());
        }
        for desc in result.source.descriptions {
            match desc {
                ast::Description::Module(m) => {
                    let name = m.name.name.clone();
                    if !lib.contains_key(&name) {
                        lib.insert(name.clone(), SourceDefinition::Module(Rc::new(m)));
                        lib_origins.insert(name, (path.clone(), explicit_v, "module"));
                    }
                }
                ast::Description::Interface(i) => {
                    let name = i.name.name.clone();
                    if !lib.contains_key(&name) {
                        lib.insert(name.clone(), SourceDefinition::Interface(Rc::new(i)));
                        lib_origins.insert(name, (path.clone(), explicit_v, "interface"));
                    }
                }
                ast::Description::Program(p) => {
                    let name = p.name.name.clone();
                    if !lib.contains_key(&name) {
                        lib.insert(name.clone(), SourceDefinition::Program(Rc::new(p)));
                        lib_origins.insert(name, (path.clone(), explicit_v, "program"));
                    }
                }
                // A non-forward typedef may fill a forward typedef the primary
                // design actually declared; adopted below, never blanket. A
                // forward typedef, class or package is never pulled from a
                // library dir (that is the scope-poisoning we avoid).
                ast::Description::TypedefDecl(t) if !t.forward => {
                    lib_typedefs.push(Rc::new(t));
                }
                // §29: a UDP defined in a `-v`/`-y` library file (vendor
                // stdcell). Adopted on demand like a library module.
                ast::Description::Udp(u) => {
                    let name = u.name.name.clone();
                    if !lib.contains_key(&name) {
                        lib.insert(name.clone(), SourceDefinition::Udp(Rc::new(u)));
                        lib_origins.insert(name, (path.clone(), explicit_v, "UDP"));
                    }
                }
                _ => {}
            }
        }
    }

    // Instantiated module/interface/program names inside a definition's body.
    fn instantiations(def: &SourceDefinition, out: &mut std::collections::HashSet<String>) {
        let items = match def {
            SourceDefinition::Module(m) => &m.items,
            SourceDefinition::Interface(i) => &i.items,
            SourceDefinition::Program(p) => &p.items,
            _ => return,
        };
        collect_instantiated_modules(items, out);
    }

    // Adopt only library modules that satisfy an unresolved instantiation
    // reachable from the explicitly-compiled design, transitively (a pulled-in
    // library module may itself instantiate further library modules).
    let mut seed = std::collections::HashSet::new();
    // Instantiated-name -> referring definition names. This is what turns an
    // opaque "module 'X' not found" into an actionable note: it names WHICH
    // module's body references X, so a user can check whether that reference
    // even elaborates (a reference inside a dead `generate`/parameter branch
    // is collected by this TEXTUAL scan but never needed at runtime — a
    // commercial elaborator would report nothing for it).
    let mut referrers: std::collections::HashMap<String, std::collections::BTreeSet<String>> =
        std::collections::HashMap::new();
    for (def_name, def) in definitions.iter() {
        let mut names = std::collections::HashSet::new();
        instantiations(def, &mut names);
        for n in &names {
            referrers.entry(n.clone()).or_default().insert(def_name.clone());
        }
        seed.extend(names);
    }
    let mut work: Vec<String> = seed.into_iter().collect();
    let mut unresolved: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut adopted_from_v = 0usize;
    while let Some(name) = work.pop() {
        if definitions.contains_key(&name) {
            continue;
        }
        if let Some(def) = lib.get(&name) {
            if let Some((path, true, kind)) = lib_origins.get(&name) {
                adopted_from_v += 1;
                if lib_cli.primitive_verbose {
                    eprintln!(
                        "[primitive-verbose] adopting {} '{}' from -v '{}' to resolve an instantiation",
                        kind,
                        name,
                        path.display()
                    );
                }
            }
            let mut more = std::collections::HashSet::new();
            instantiations(def, &mut more);
            definitions.insert(name.clone(), def.clone());
            for n in more {
                referrers.entry(n.clone()).or_default().insert(name.clone());
                if !definitions.contains_key(&n) {
                    work.push(n);
                }
            }
        } else {
            unresolved.insert(name);
        }
    }
    if lib_cli.primitive_verbose && !lib_cli.lib_files.is_empty() {
        let indexed_from_v = lib_origins.values().filter(|(_, from_v, _)| *from_v).count();
        eprintln!(
            "[primitive-verbose] -v resolution summary: files={} indexed_definitions={} adopted={} unresolved={}",
            lib_cli.lib_files.len(),
            indexed_from_v,
            adopted_from_v,
            unresolved.len()
        );
        for name in &unresolved {
            eprintln!(
                "[primitive-verbose] unresolved definition '{}' after scanning explicit -v files",
                name
            );
        }
    }
    // Detailed context for names the libraries could not supply — printed here,
    // where the library scan is in scope, so the eventual "instantiated but not
    // found" elaboration error arrives with its cause already on the terminal.
    if !unresolved.is_empty() && (!lib.is_empty() || !lib_cli.lib_files.is_empty()) {
        // Print EVERY unresolved name — a truncated list once hid 2 of a
        // customer design's 10 missing vendor cells.
        for name in unresolved.iter() {
            let mut line = format!(
                "note: module '{}' not defined in the design and not found among {} definition(s) indexed from {} library file(s)",
                name,
                lib.len(),
                scanned_paths.len()
            );
            // WHO references it — so the user can judge whether the reference
            // is live (a real missing cell) or sits in a branch elaboration
            // never enters (in which case this note is advisory only; the
            // textual scan cannot evaluate generate/parameter conditions).
            if let Some(refs) = referrers.get(name) {
                let shown: Vec<String> = refs
                    .iter()
                    .take(4)
                    .map(|r| match lib_origins.get(r) {
                        Some((path, _, _)) => format!("'{}' ({})", r, path.display()),
                        None => format!("'{}'", r),
                    })
                    .collect();
                line.push_str(&format!(" — instantiated in: {}", shown.join(", ")));
                if refs.len() > 4 {
                    line.push_str(&format!(" and {} more", refs.len() - 4));
                }
                line.push_str(
                    "; if that reference sits in a generate/`ifdef branch that never elaborates, this note is advisory and no model is needed",
                );
            }
            // Case mismatch is a classic netlist/lib mismatch.
            if let Some(close) = lib
                .keys()
                .find(|k| k.eq_ignore_ascii_case(name) && k.as_str() != name.as_str())
            {
                line.push_str(&format!(
                    " — did you mean '{}'? (module names are case-sensitive)",
                    close
                ));
            }
            // Primitive-looking text without an indexed UDP usually means the
            // parser lost the declaration after an earlier syntax error.
            let prim_pat = format!("primitive {}", name);
            let prim_pat2 = format!("primitive  {}", name);
            if let Some(f) = scanned_paths.iter().find(|p| {
                std::fs::read_to_string(p)
                    .map(|s| s.contains(&prim_pat) || s.contains(&prim_pat2))
                    .unwrap_or(false)
            }) {
                line.push_str(&format!(
                    " — '{}' contains a UDP `primitive` with this name, but parsing did not recover its definition; rerun with --primitive-verbose",
                    f.display()
                ));
            }
            eprintln!("{}", line);
        }
        for f in parse_issue_files.iter().take(3) {
            eprintln!(
                "note: '{}' had parse errors (see warnings above) — its definitions may be incomplete",
                f.display()
            );
        }
    }

    // §6.18: fill a forward typedef the primary design declared (`typedef
    // name;`) from a library file's real typedef — only those, never blanket.
    for t in lib_typedefs {
        let name = t.name.name.clone();
        let replace_forward = matches!(
            definitions.get(&name),
            Some(SourceDefinition::Typedef(e)) if e.forward);
        if replace_forward {
            definitions.insert(name, SourceDefinition::Typedef(t));
        }
    }
    Ok(())
}

/// Set the log file for simulation output. Placeholder.
pub fn log_println(s: &str) { println!("{}", s); }
pub fn log_eprintln(s: &str) { eprintln!("{}", s); }
