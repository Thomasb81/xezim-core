# SV-2023 development log (xezim-core)

Detailed per-change history that was squashed into one summary commit on this branch. Reflects work done between `a9173da` and the squash.

## Commits (in order)


### 0822eaf parser: SV-2023 syntax extensions behind a gate


Add a process-wide `is_sv2023()` flag (off by default) and wire three
IEEE 1800-2023 syntax additions behind it:

  - §5.9  triple-quoted string literals (""" ... """)
  - §13.5.2 `ref static` task/function args

The 2017 lex/parse path is unchanged when the gate is off.




### 8408db6 parser+elab: SV-2023 type-param extends, final methods


Parser (gated on `--sv2023`):

  - §6.20.2.1  `parameter type T extends Base` — TypeParamAssignment
                gains `extends: Option<Identifier>` (constraint parsed,
                not yet enforced at type-arg binding time).
  - §8.20.5    `final` qualifier on class methods — new
                ClassQualifier::Final, accepted in parse_class_item
                when followed by another qualifier or function/task
                (disambiguated from `final` *blocks*).

Elaboration (gated on is_sv2023()):

  - §8.20.5 enforcement: validate_final_method_overrides walks each
    class's ancestor chain; redeclaring an ancestor's `final` method
    fails elaboration.




### 8ce4eb5 parser: SV-2023 end-label mismatch detection


IEEE 1800-2023 §27.2.1: a labelled `endmodule : <name>` (or
`endfunction : <name>`) must agree with the declared name. New
parse_end_label_checked helper emits a diagnostic on mismatch,
gated on the SV-2023 mode flag. Wired into module and function
declarations; other end-label sites still use the unchecked
variant and are a follow-up.




### 3cccdb0 SV-2023 colon-specifiers, escape decode, timeunits, more


Replace the wrong C++-style `final` qualifier with the proper
IEEE 1800-2023 §8.20.5 colon-specifier syntax:

  - `class :final <name>` — class cannot be extended
  - `function :final|:extends|:initial <ret> <name>`
  - `task :final|:extends|:initial <name>`

AST gains MethodSpecifier (Final/Extends/Initial) and is_final on
ClassDeclaration; the dead ClassQualifier::Final variant is removed.
Enforcement rejects (a) extending a `:final` class, (b) overriding a
`:final` method anywhere in the ancestor chain.

Additional SV-2023 / generic fixes:

  - String literal escape decoding (`\\n`, `\\t`, `\\"`, `\\xHH`,
    octal). Pre-existing parser bug; affected triple-quoted t100.
  - `$timeunit`/`$timeprecision`: new ModuleItem::TimeunitsDecl
    variant, captured at parse time and resolved to a
    log10-second exponent stored on ElaboratedModule.
  - `inside { [A +/- B] }` and `[A +%- B]` tolerance ranges
    (§11.4.13) parsed as `[A-B : A+B]` / scaled form.
  - `always_ff` body: reject nested timing/event controls
    (§9.2.2.4).
  - Duplicate module definition: error at description ingestion
    (§3.3).




### e391332 parser/elab: array-method `with`-clause iterator binding


`validate_expr_idents` for `WithClause` now treats the receiver
Call's arguments as iterator-name bindings rather than value
references, and adds them to the filter's local scope. Lets
`arr.find(x) with (x...)` / `arr.find(item, idx) with (...)` and
`arr.map(x) with (...)` (IEEE 1800-2023) elaborate without a
spurious "Undeclared identifier" error.




### 4c1645c parser: AssignmentPatternItem::Keyed for string-keyed aggregates


IEEE 1800-2023 §10.10: associative-array assignment patterns admit
non-identifier keys, e.g. `'{"HIGH": 5, "LOW": 1}`. New `Keyed`
variant on AssignmentPatternItem and parser support for
`<string-literal> ':' <expr>` items.




### ecb3628 parser: type(this) typeof operator (SV-2023 §6.20.2.1)


Class declaration parser maintains a thread-local class-name
stack while parsing class bodies. When the data-type parser sees
`type(this)` in SV-2023 mode it resolves to a TypeReference for
the enclosing class. Other `type(<expr>)` forms parse but fall
back to an Implicit type for now.

Class-item dispatcher also recognises `type(` as the start of a
property declaration, so `static type(this) singleton;` parses
where it previously errored.




### 2985426 elab: register packed-struct field layout for arrays/queues


Data declarations with an array dimension (incl. queue / dynamic
array) and a packed-struct element type (directly or via a
typedef) now also populate `packed_struct_fields` under the
array name. Previously only scalar struct declarations got the
layout, so `arr[i].field` slices failed for queues-of-struct.




### 7c3b978 preprocessor+elab: associative-array typed parameters


Preprocessor: macro-arg substitution now skips occurrences inside
string literals in the macro body. Previously, an `actual` arg
containing `"` (e.g. `WEIGHT["HIGH"]`) would corrupt a format
string like `"actual=%0d"` because the substring `actual` was
substituted regardless of context.

Elaboration: parameter init expressions of the form
`'{ "k": v, "k": v, ... }` are now recognised as associative-
array initialisers. Materialised at both module-level and
sub-module inlining paths: registers `<param>` in
`associative_arrays` and inserts `<prefix><param>["k"]` signals
for each entry. Lets `WEIGHT["HIGH"]` on an assoc-array typed
parameter resolve to the supplied default value.




### 14e4fe0 elab: reject non-lvalue args to `ref` formals (1800-2017 §13.5.2)


`ref` task/function arguments must be variables — they're passed
by name and the callee can write through them. A literal or a
binary expression isn't an lvalue and must be a compile-time
error.

New `validate_ref_arg_lvalues` pass walks every initial/always
block plus function and task bodies, finds plain Call sites with
an identifier callee, looks up the formal port list on the
matching task/function, and checks each `ref` formal's arg with
a syntactic lvalue predicate (Ident/Index/RangeSelect/
MemberAccess/Concatenation).




### 3ffb26b preprocessor: preserve trailing whitespace in macro args


`extract_macro_args` was calling `.trim()` on each arg slice,
stripping the trailing space that terminates a SystemVerilog
escaped identifier (1800-2017 §22.5). When the arg was something
like `\a.b ` (the trailing space matters), the substitution
would emit `\a.b` adjacent to a closing `)` and the lexer's
greedy `\<id>` scan ate the `)` as part of the identifier.

Trim only leading whitespace plus internal-format chars
(\n, \r, \t) but keep ordinary trailing spaces. Resolves t012
(escaped identifier) in the bundled testsuite without regressing
any other suite.




### 004b6d9 lexer: time-unit suffix on real literals (1800-2017 §3.14.2)


`1.250ns` was tokenised as RealLiteral followed by Identifier
'ns', leaving downstream parsers to see an undeclared identifier.
The integer path already attached `ns`/`us`/`ms`/`ps`/`fs`/`s`
suffixes to produce a TimeLiteral; mirror the same logic at the
end of the real-literal path. Lets the parser see a single
TimeLiteral for fractional time literals.




### e343630 elab: track enum typedef members in declaration order


Populate a new `enum_members: HashMap<String, Vec<(String, u64)>>`
on `ElaboratedModule` when processing an enum typedef, keyed by
typedef name. Also set `Signal.type_name` to the typedef name on
the per-member signals.

Foundation for `.name()` / `.next()` / `.first()` / `.last()` /
`.prev()` enum methods (IEEE 1800-2017 §6.19.5). The dispatch
side needs `Signal.type_name` to survive across the inline_module
items pass on enum-typed signals declared by the user — that
plumbing isn't quite there yet, so the methods stay unimplemented
for now.

#[serde(default)] keeps existing bincode artifacts deserialisable.




### 34d1819 elab: reject non-member enum assignments (1800-2017 §6.19.3)


New `validate_enum_assignments` pass walks every initial/always
block plus function and task bodies, checks each
BlockingAssign/NonblockingAssign whose lvalue is a plain
identifier of an enum-typed signal, and rejects literal RHS
values that don't appear in the typedef's declared member list.
Conservative — only fully-constant rvalues are folded; anything
involving an identifier or call is skipped to avoid spurious
errors.




### c69f548 elab: expand gate primitives at module top level


The top-level module-items loop in elaborate_module_with_defs
silently dropped `GateInstantiation` AST nodes (the loop only
handled the form within generate regions via `elaborate_items`).
Module-level `and`/`or`/`xor`/`nand`/`nor`/`xnor`/`not`/`buf`
gates therefore never drove their outputs.

Synthesise the equivalent continuous assigns at the top loop,
matching the generate-region behavior. Extended
`gate_inst_to_assign_pairs` to cover xor / nand / nor / xnor /
buf in addition to and / or / not.

Fixes t091 (gate primitives) in the SV-2023 testsuite.




### 0242fa3 preprocessor+elab: reject implicit nets under default_nettype none


The preprocessor previously discarded every `default_nettype`
directive. Now it parses the argument: when it sees `none` the
parser sets a per-source sticky flag (reset at the start of each
`preprocess` call so files in the same process don't pollute
each other). The elaborator's `create_implicit_nets` pass
consults the flag and returns a compile error rather than
auto-creating a 1-bit net under the `none` region (IEEE
1800-2017 §6.10).

Conservative: the flag is sticky for the lifetime of one
preprocess pass, so a later `` `default_nettype wire `` in the
same file doesn't re-enable implicit nets there. None of the
existing bundled tests mix the two states.

Fixes n001 in the SV-2023 negative tests.




### 71b9fe6 elab: track 2-state-typed signals for assignment coercion


New `two_state_signals: HashSet<String>` on `ElaboratedModule`,
populated by the DataDeclaration handler whenever the signal's
declared type is 2-state (bit, byte, shortint, int, longint,
etc., per the existing `is_type_two_state` predicate).

The simulator consults the set on every assignment and masks
any X/Z source bit to 0 when the target is registered, matching
IEEE 1800-2017 §6.3 (coercion at the assignment boundary).




### 51e071b parser: emit StatementKind::DisableFork for `disable fork`


Previously `disable fork;` was parsed as `StatementKind::Null`,
discarding the keyword entirely. New variant preserves the
intent so the simulator can act on it.




### f96cc48 lexer/parser: time literals → Real(seconds)


`1.250ns`, `100ps`, etc. now parse cleanly into NumberLiteral::Real
with the value scaled to seconds (e.g. 1.25e-9 for 1.250ns). The
previous behaviour fell through to the based-literal check, then
to a plain-decimal store that kept the unit characters in the
mantissa string and broke downstream consumers.

The simulator's delay handler converts back to time units using
the active timeunit_exp, so fractional delays like #1.250ns
advance the new real_time tracker by the precise amount.




### 1e51351 parser: accept and discard top-level `bind` directives


IEEE 1800-2017 §23.11 binds an instance of one module into
every instance of another, typically used to attach checkers
or monitors to a design without modifying it. The parser
previously errored on the `bind` keyword at top level.

Parse the directive (skipping balanced parens and consuming up
to the terminating semicolon) and continue. The monitor body is
not instantiated for now; the directive is structurally
recognised so designs that use it compile and the underlying
target module runs unchanged.

Fixes t065 in the SV-2023 testsuite. Per-target injection of the
bound instance is a deeper elaboration change left as follow-up.



