//! TypeScript macro compilation for the macro Worker (and node in tests).
//!
//! Go compiled with esbuild (bundle, CommonJS, ES2020, `@recur/sdk` as the
//! only resolvable import). This port uses swc in-process and reproduces
//! the observable contract:
//!
//! - TypeScript is stripped with esbuild's defaults (unused and type-only
//!   imports dropped, define semantics for class fields).
//! - Syntax newer than ES2020 is lowered (class fields, static blocks,
//!   `#x in obj`, logical assignment).
//! - ESM becomes CommonJS on `module.exports` / `exports`, run by the
//!   Worker's `new AsyncFunction("ctx", "const module = …" + code)` boot.
//! - `@recur/sdk` is compiled once and inlined where the entry requires it
//!   (esbuild's bundling); every other import, `require`, or dynamic
//!   `import()` with a literal path fails with esbuild's plugin message,
//!   located at the path literal. Named imports the SDK does not export
//!   fail with esbuild's "No matching export" message.
//! - Top-level await fails as it does for esbuild's cjs format.
//!
//! The output text differs from esbuild's; behavior is identical (proven
//! over every shipped TypeScript file by `tests/compile_parity.rs`).

use std::collections::BTreeSet;
use std::sync::LazyLock;

use sha2::{Digest, Sha256};
use swc_core::common::comments::SingleThreadedComments;
use swc_core::common::sync::Lrc;
use swc_core::common::{FileName, GLOBALS, Globals, Mark, SourceMap, Span, Spanned, SyntaxContext};
use swc_core::ecma::ast::*;
use swc_core::ecma::codegen::text_writer::JsWriter;
use swc_core::ecma::codegen::{Config as CodegenConfig, Emitter};
use swc_core::ecma::parser::{Syntax, TsSyntax, parse_file_as_program};
use swc_core::ecma::transforms::base::fixer::fixer;
use swc_core::ecma::transforms::base::helpers::{HELPERS, Helpers, inject_helpers};
use swc_core::ecma::transforms::base::hygiene::hygiene;
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::transforms::compat::{es2021, es2022};
use swc_core::ecma::transforms::module::common_js::{FeatureFlag, common_js};
use swc_core::ecma::transforms::module::path::Resolver;
use swc_core::ecma::transforms::module::util::Config as ModuleConfig;
use swc_core::ecma::transforms::typescript::{Config as TsConfig, typescript};
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

use crate::go::{self, quote};
use crate::manifest::{Command, INTERPRETER_TYPESCRIPT, PromptAction};
use crate::{Error, err};

/// Bumped when the SDK or Worker contract changes so cached JS is
/// invalidated (`macro_sdk.ts`, `frontend/src/lib/macro-worker.ts`).
pub const MACRO_SDK_VERSION: &str = "8";

/// The import path authors use in TypeScript commands.
pub const MACRO_SDK_MODULE: &str = "@recur/sdk";

/// The `@recur/sdk` module body, embedded from its Go home as is.
/// Author-facing copy: `bundles/sdk/index.ts` (kept in sync; see tests).
pub const MACRO_SDK_SOURCE: &str = include_str!("../../../../internal/bundles/macro_sdk.ts");

/// Whether `interp` runs as compiled TypeScript in the frontend Worker
/// rather than as a shebang subprocess.
pub fn is_script_interpreter(interp: &str) -> bool {
    interp == INTERPRETER_TYPESCRIPT
}

/// Compiled CommonJS ready for the macro Worker.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompileResult {
    /// CommonJS source with a default export.
    pub js: String,
    pub cache_hit: bool,
    pub hash: String,
}

/// Transforms a TypeScript command body to CommonJS for the Worker.
/// Results are cached under `cache_dir` as `<id>.<hash>.js`.
pub fn compile_macro(cache_dir: &str, cmd: &Command) -> Result<CompileResult, Error> {
    let id = quote(&cmd.id);
    if !is_script_interpreter(&cmd.interpreter) {
        return Err(err!(
            "command {id}: interpreter {} is not typescript",
            quote(&cmd.interpreter)
        ));
    }
    if cmd.body.trim().is_empty() {
        return Err(err!("command {id}: empty typescript source"));
    }
    let src = ensure_default_export(&cmd.body);
    let key = format!(
        "{MACRO_SDK_VERSION}\0{}\0{MACRO_SDK_SOURCE}\0{src}",
        cmd.interpreter
    );
    compile_cached(cache_dir, &cmd.id, &key, &src, &format!("compile {id}"))
}

/// Transforms a TypeScript prompt-action body to CommonJS. Same cache layout
/// as [`compile_macro`].
pub fn compile_prompt_macro(
    cache_dir: &str,
    prompt: &PromptAction,
) -> Result<CompileResult, Error> {
    let id = quote(&prompt.id);
    if !is_script_interpreter(&prompt.interpreter) {
        return Err(err!(
            "prompt {id}: interpreter {} is not typescript",
            quote(&prompt.interpreter)
        ));
    }
    if prompt.body.trim().is_empty() {
        return Err(err!("prompt {id}: empty typescript source"));
    }
    let src = ensure_prompt_default_export(&prompt.body);
    let key = format!(
        "{MACRO_SDK_VERSION}\0prompt\0{}\0{MACRO_SDK_SOURCE}\0{src}",
        prompt.interpreter
    );
    compile_cached(
        cache_dir,
        &prompt.id,
        &key,
        &src,
        &format!("compile prompt {id}"),
    )
}

/// Compiles a TypeScript `[[prompt]]` complete script.
pub fn compile_complete_macro(
    cache_dir: &str,
    prompt: &PromptAction,
) -> Result<CompileResult, Error> {
    let id = quote(&prompt.id);
    if prompt.complete_interpreter != INTERPRETER_TYPESCRIPT {
        return Err(err!(
            "prompt {id}: complete interpreter {} is not typescript",
            quote(&prompt.complete_interpreter)
        ));
    }
    if prompt.complete_body.trim().is_empty() {
        return Err(err!("prompt {id}: empty complete source"));
    }
    let src = ensure_complete_default_export(&prompt.complete_body);
    let key = format!("{MACRO_SDK_VERSION}\0prompt-complete\0{MACRO_SDK_SOURCE}\0{src}");
    compile_cached(
        cache_dir,
        &format!("{}-complete", prompt.id),
        &key,
        &src,
        &format!("compile complete {id}"),
    )
}

fn compile_cached(
    cache_dir: &str,
    id: &str,
    key: &str,
    src: &str,
    what: &str,
) -> Result<CompileResult, Error> {
    let sum = Sha256::digest(key.as_bytes());
    let hash = hex::encode(&sum[..16]);
    let cache_path = go::join(&[cache_dir, &format!("{id}.{hash}.js")]);
    if let Ok(data) = std::fs::read(&cache_path)
        && !data.is_empty()
    {
        return Ok(CompileResult {
            js: String::from_utf8_lossy(&data).into_owned(),
            cache_hit: true,
            hash,
        });
    }
    let js = build(src, &format!("{id}.ts")).map_err(|msgs| err!("{what}: {}", msgs.join("; ")))?;
    if js.trim().is_empty() {
        return Err(err!("{what}: empty output"));
    }
    std::fs::create_dir_all(cache_dir)
        .map_err(|e| err!("{}", go::path_err("mkdir", cache_dir, &e)))?;
    let _ = crate::seed::write_file(&cache_path, js.as_bytes(), 0o644);
    prune_old_cache(cache_dir, id, &hash);
    Ok(CompileResult {
        js,
        cache_hit: false,
        hash,
    })
}

fn has_own_export(src: &str) -> bool {
    let trim = src.trim();
    trim.contains("export default") || trim.contains("module.exports")
}

/// Wraps a bare statement body so authors can write either a full module or
/// a concise body; bare bodies get a default `@recur/sdk` import.
fn ensure_default_export(src: &str) -> String {
    if has_own_export(src) {
        return src.to_string();
    }
    format!(
        "import {{ Output, replaceEdited, type MacroContext, type MacroResult }} from \"{MACRO_SDK_MODULE}\"\n\
         export default async function (ctx: MacroContext): Promise<MacroResult> {{\n{src}\n}}\n"
    )
}

fn ensure_prompt_default_export(src: &str) -> String {
    if has_own_export(src) {
        return src.to_string();
    }
    format!(
        "import {{ type PromptContext, type PromptResult }} from \"{MACRO_SDK_MODULE}\"\n\
         export default async function (ctx: PromptContext): Promise<PromptResult> {{\n{src}\n}}\n"
    )
}

fn ensure_complete_default_export(src: &str) -> String {
    if has_own_export(src) {
        return src.to_string();
    }
    format!(
        "import {{ type PromptCompleteContext, type PromptCompleteResult }} from \"{MACRO_SDK_MODULE}\"\n\
         export default async function (ctx: PromptCompleteContext): Promise<PromptCompleteResult> {{\n{src}\n}}\n"
    )
}

fn prune_old_cache(cache_dir: &str, id: &str, keep_hash: &str) {
    let Ok(entries) = std::fs::read_dir(cache_dir) else {
        return;
    };
    let prefix = format!("{id}.");
    let keep = format!("{id}.{keep_hash}.js");
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with(&prefix) && name.ends_with(".js") && name != keep {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// The SDK compiled to a CommonJS body plus its runtime export names.
struct CompiledSdk {
    js: String,
    exports: BTreeSet<String>,
}

static SDK: LazyLock<Result<CompiledSdk, String>> = LazyLock::new(|| {
    let mut exports = BTreeSet::new();
    let js = transform(
        MACRO_SDK_SOURCE,
        "recur-sdk:@recur/sdk",
        None,
        Some(&mut exports),
    )
    .map_err(|msgs| msgs.join("; "))?;
    Ok(CompiledSdk { js, exports })
});

/// Compiles an entry module, inlining the SDK when the entry requires it.
fn build(src: &str, sourcefile: &str) -> Result<String, Vec<String>> {
    let sdk = SDK.as_ref().map_err(|e| vec![format!("@recur/sdk: {e}")])?;
    transform(src, sourcefile, Some(sdk), None)
}

/// One TypeScript → CommonJS pass. With `sdk`, imports are checked against
/// it and inlined; without, the source is the SDK itself and its runtime
/// exports are collected into `exports_out`.
fn transform(
    src: &str,
    sourcefile: &str,
    sdk: Option<&CompiledSdk>,
    exports_out: Option<&mut BTreeSet<String>>,
) -> Result<String, Vec<String>> {
    GLOBALS.set(&Globals::new(), || {
        HELPERS.set(&Helpers::new(false), || {
            transform_inner(src, sourcefile, sdk, exports_out)
        })
    })
}

fn transform_inner(
    src: &str,
    sourcefile: &str,
    sdk: Option<&CompiledSdk>,
    exports_out: Option<&mut BTreeSet<String>>,
) -> Result<String, Vec<String>> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        Lrc::new(FileName::Custom(sourcefile.to_string())),
        src.to_string(),
    );
    let locate = |span: Span| -> String {
        let offset = (span.lo.0.saturating_sub(fm.start_pos.0)) as usize;
        let before = &src.as_bytes()[..offset.min(src.len())];
        let line = before.iter().filter(|b| **b == b'\n').count() + 1;
        let col = before.len()
            - before
                .iter()
                .rposition(|b| *b == b'\n')
                .map_or(0, |i| i + 1);
        format!("{sourcefile}:{line}:{col}")
    };

    let comments = SingleThreadedComments::default();
    let mut recovered = Vec::new();
    let syntax = Syntax::Typescript(TsSyntax::default());
    let mut program = match parse_file_as_program(
        &fm,
        syntax,
        EsVersion::latest(),
        Some(&comments),
        &mut recovered,
    ) {
        Ok(p) => p,
        Err(e) => {
            recovered.push(e);
            return Err(recovered
                .iter()
                .map(|e| format!("{}: {}", locate(e.span()), e.kind().msg()))
                .collect());
        }
    };
    if !recovered.is_empty() {
        return Err(recovered
            .iter()
            .map(|e| format!("{}: {}", locate(e.span()), e.kind().msg()))
            .collect());
    }

    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();
    program.mutate(resolver(unresolved_mark, top_level_mark, true));
    let ts_config = TsConfig {
        native_class_properties: true,
        ..Default::default()
    };
    program.mutate(typescript(ts_config, unresolved_mark, top_level_mark));

    // esbuild resolves imports after dropping unused TypeScript imports.
    let mut check = ImportCheck {
        unresolved: SyntaxContext::empty().apply_mark(unresolved_mark),
        sdk_exports: sdk.map(|s| &s.exports),
        errors: Vec::new(),
    };
    program.visit_with(&mut check);
    let mut tla = TopLevelAwait::default();
    program.visit_with(&mut tla);
    let mut errors: Vec<(Span, String)> = check.errors;
    errors.extend(tla.found.into_iter().map(|span| {
        (
            span,
            "Top-level await is currently not supported with the \"cjs\" output format".to_string(),
        )
    }));
    if !errors.is_empty() {
        errors.sort_by_key(|(span, _)| span.lo);
        return Err(errors
            .into_iter()
            .map(|(span, msg)| format!("{}: {msg}", locate(span)))
            .collect());
    }
    if let Some(out) = exports_out {
        collect_exports(&program, out);
    }

    program.mutate(ImportMetaToEmpty);
    program.mutate(es2022::static_blocks());
    program.mutate(es2022::private_in_object());
    program.mutate(es2022::class_properties(
        es2022::class_properties::Config {
            private_as_properties: false,
            set_public_fields: false,
            constant_super: false,
            no_document_all: false,
            pure_getter: false,
        },
        unresolved_mark,
    ));
    program.mutate(es2021::es2021());
    program.mutate(common_js(
        Resolver::Default,
        unresolved_mark,
        ModuleConfig {
            allow_top_level_this: false,
            ..Default::default()
        },
        FeatureFlag {
            support_block_scoping: true,
            support_arrow: true,
        },
    ));
    let mut inline = InlineSdk {
        unresolved: SyntaxContext::empty().apply_mark(unresolved_mark),
        name: sdk_binding_name(src),
        used: false,
    };
    if sdk.is_some() {
        program.visit_mut_with(&mut inline);
    }
    program.mutate(inject_helpers(unresolved_mark));
    program.mutate(hygiene());
    program.mutate(fixer(Some(&comments)));

    let mut buf = Vec::new();
    {
        let mut emitter = Emitter {
            cfg: CodegenConfig::default().with_target(EsVersion::Es2020),
            cm: cm.clone(),
            comments: None,
            wr: JsWriter::new(cm.clone(), "\n", &mut buf, None),
        };
        emitter
            .emit_program(&program)
            .map_err(|e| vec![format!("{sourcefile}: {e}")])?;
    }
    let code = String::from_utf8_lossy(&buf).into_owned();
    match sdk {
        Some(sdk) if inline.used => Ok(format!(
            "var {} = (function () {{\nvar exports = {{}};\nvar module = {{ exports: exports }};\n{}\nreturn module.exports;\n}})();\n{code}",
            inline.name, sdk.js
        )),
        _ => Ok(code),
    }
}

/// A binding name for the inlined SDK that cannot collide with the source.
fn sdk_binding_name(src: &str) -> String {
    let mut name = "__recur_sdk".to_string();
    while src.contains(&name) {
        name.push('_');
    }
    name
}

/// esbuild's resolve-time rules for a bundled macro.
struct ImportCheck<'a> {
    unresolved: SyntaxContext,
    sdk_exports: Option<&'a BTreeSet<String>>,
    errors: Vec<(Span, String)>,
}

impl ImportCheck<'_> {
    fn resolve(&mut self, src: &Str) -> bool {
        let path = src.value.to_string_lossy();
        if path == MACRO_SDK_MODULE {
            return true;
        }
        self.errors.push((
            src.span,
            format!(
                "import {} is not allowed (only {})",
                quote(&path),
                quote(MACRO_SDK_MODULE)
            ),
        ));
        false
    }

    fn require_export(&mut self, span: Span, name: &str) {
        if let Some(exports) = self.sdk_exports
            && !exports.contains(name)
        {
            self.errors.push((
                span,
                format!(
                    "No matching export in \"recur-sdk:{MACRO_SDK_MODULE}\" for import {}",
                    quote(name)
                ),
            ));
        }
    }

    fn literal_arg(args: &[ExprOrSpread]) -> Option<&Str> {
        match args.first().map(|a| &*a.expr) {
            Some(Expr::Lit(Lit::Str(s))) if args.len() == 1 => Some(s),
            _ => None,
        }
    }
}

fn module_export_name(n: &ModuleExportName) -> String {
    match n {
        ModuleExportName::Ident(i) => i.sym.to_string(),
        ModuleExportName::Str(s) => s.value.to_string_lossy().into_owned(),
    }
}

impl Visit for ImportCheck<'_> {
    fn visit_import_decl(&mut self, n: &ImportDecl) {
        if n.type_only || !self.resolve(&n.src) {
            return;
        }
        for spec in &n.specifiers {
            match spec {
                ImportSpecifier::Named(s) if !s.is_type_only => {
                    let name = s
                        .imported
                        .as_ref()
                        .map_or_else(|| s.local.sym.to_string(), module_export_name);
                    self.require_export(s.span, &name);
                }
                ImportSpecifier::Default(d) => self.require_export(d.span, "default"),
                _ => {}
            }
        }
    }

    fn visit_named_export(&mut self, n: &NamedExport) {
        let Some(src) = &n.src else { return };
        if n.type_only || !self.resolve(src) {
            return;
        }
        for spec in &n.specifiers {
            if let ExportSpecifier::Named(s) = spec
                && !s.is_type_only
            {
                self.require_export(s.span, &module_export_name(&s.orig));
            }
        }
    }

    fn visit_export_all(&mut self, n: &ExportAll) {
        if !n.type_only {
            self.resolve(&n.src);
        }
    }

    fn visit_call_expr(&mut self, n: &CallExpr) {
        match &n.callee {
            Callee::Import(_) => {
                if let Some(s) = Self::literal_arg(&n.args) {
                    self.resolve(s);
                }
            }
            Callee::Expr(e) => {
                let is_require = |e: &Expr| matches!(e, Expr::Ident(i) if i.sym == "require" && i.ctxt == self.unresolved);
                let resolves = match &**e {
                    e if is_require(e) => true,
                    Expr::Member(m) => {
                        is_require(&m.obj)
                            && matches!(&m.prop, MemberProp::Ident(p) if p.sym == "resolve")
                    }
                    _ => false,
                };
                if resolves && let Some(s) = Self::literal_arg(&n.args) {
                    self.resolve(s);
                }
            }
            Callee::Super(_) => {}
        }
        n.visit_children_with(self);
    }
}

/// Finds `await` / `for await` outside any function.
#[derive(Default)]
struct TopLevelAwait {
    found: Vec<Span>,
}

impl Visit for TopLevelAwait {
    fn visit_await_expr(&mut self, n: &AwaitExpr) {
        self.found.push(n.span);
        n.visit_children_with(self);
    }
    fn visit_for_of_stmt(&mut self, n: &ForOfStmt) {
        if n.is_await {
            self.found.push(n.span);
        }
        n.visit_children_with(self);
    }
    fn visit_function(&mut self, _: &Function) {}
    fn visit_arrow_expr(&mut self, _: &ArrowExpr) {}
    fn visit_class(&mut self, c: &Class) {
        // Field initializers and static blocks are not async contexts, but
        // methods are functions; only look at the heritage clause.
        if let Some(sc) = &c.super_class {
            sc.visit_with(self);
        }
    }
}

/// Runtime export names of a module (the SDK).
fn collect_exports(program: &Program, out: &mut BTreeSet<String>) {
    let Program::Module(m) = program else { return };
    for item in &m.body {
        let ModuleItem::ModuleDecl(decl) = item else {
            continue;
        };
        match decl {
            ModuleDecl::ExportDecl(ExportDecl { decl, .. }) => match decl {
                Decl::Fn(f) => {
                    out.insert(f.ident.sym.to_string());
                }
                Decl::Class(c) => {
                    out.insert(c.ident.sym.to_string());
                }
                Decl::Var(v) => {
                    for d in &v.decls {
                        if let Pat::Ident(i) = &d.name {
                            out.insert(i.id.sym.to_string());
                        }
                    }
                }
                _ => {}
            },
            ModuleDecl::ExportNamed(n) => {
                for spec in &n.specifiers {
                    if let ExportSpecifier::Named(s) = spec {
                        out.insert(module_export_name(s.exported.as_ref().unwrap_or(&s.orig)));
                    }
                }
            }
            ModuleDecl::ExportDefaultDecl(_) | ModuleDecl::ExportDefaultExpr(_) => {
                out.insert("default".to_string());
            }
            _ => {}
        }
    }
}

/// esbuild's cjs output replaces `import.meta` with an empty object.
struct ImportMetaToEmpty;

impl Pass for ImportMetaToEmpty {
    fn process(&mut self, program: &mut Program) {
        program.visit_mut_with(self);
    }
}

impl VisitMut for ImportMetaToEmpty {
    fn visit_mut_expr(&mut self, e: &mut Expr) {
        if let Expr::MetaProp(m) = e
            && m.kind == MetaPropKind::ImportMeta
        {
            *e = Expr::Object(ObjectLit {
                span: m.span,
                props: Vec::new(),
            });
            return;
        }
        e.visit_mut_children_with(self);
    }
}

/// Rewrites `require("@recur/sdk")` to the inlined SDK binding.
struct InlineSdk {
    unresolved: SyntaxContext,
    name: String,
    used: bool,
}

impl VisitMut for InlineSdk {
    fn visit_mut_expr(&mut self, e: &mut Expr) {
        e.visit_mut_children_with(self);
        let Expr::Call(call) = e else { return };
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let Expr::Ident(id) = &**callee else { return };
        if id.sym != "require" || id.ctxt != self.unresolved {
            return;
        }
        if let Some(s) = ImportCheck::literal_arg(&call.args)
            && s.value.to_string_lossy() == MACRO_SDK_MODULE
        {
            self.used = true;
            *e = Expr::Ident(Ident::new_no_ctxt(self.name.as_str().into(), call.span));
        }
    }
}
