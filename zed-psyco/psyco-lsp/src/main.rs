mod analysis;
mod builtins;
mod check;
mod literal;
mod stdlib;

use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fs,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::*;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;

use analysis::{Analysis, Def, DefKind, PathUse, Target, Unit, World};
use check::{check, Loader};

fn main() -> Result<(), Box<dyn Error + Sync + Send>> {
    let (connection, io_threads) = Connection::stdio();
    let capabilities = serde_json::to_value(capabilities())?;
    let init = connection.initialize(capabilities)?;

    let sender = connection.sender.clone();
    let send = move |m| {
        let _ = sender.send(m);
    };
    let mut server = Server::new(send, root_path(&init));
    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    break;
                }
                let response = server.handle_request(req);
                connection.sender.send(Message::Response(response))?;
            }
            Message::Notification(n) => server.handle_notification(n),
            Message::Response(_) => {}
        }
    }
    drop(server);
    drop(connection);
    io_threads.join()?;
    Ok(())
}

fn capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::FULL),
                save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                ..Default::default()
            },
        )),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        document_highlight_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![".".into(), ":".into()]),
            ..Default::default()
        }),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".into(), ",".into()]),
            retrigger_characters: None,
            work_done_progress_options: Default::default(),
        }),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        inlay_hint_provider: Some(OneOf::Left(true)),
        ..Default::default()
    }
}

fn root_path(init: &Value) -> Option<PathBuf> {
    init.pointer("/workspaceFolders/0/uri")
        .or_else(|| init.get("rootUri"))
        .and_then(Value::as_str)
        .and_then(uri_to_path)
}

fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let mut path = String::from_utf8(out).ok()?;
    if path.len() >= 3 && path.starts_with('/') && path.as_bytes()[2] == b':' {
        path.remove(0);
    }
    Some(PathBuf::from(path))
}

fn path_to_uri(path: &Path) -> String {
    let mut s = path.to_string_lossy().replace('\\', "/");
    if !s.starts_with('/') {
        s.insert(0, '/');
    }
    let mut out = String::from("file://");
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// File named by `include_bytes` / `include_program` in the file `from`.
fn resolve_include(from: Option<&Path>, include: &str) -> PathBuf {
    let dir = from.and_then(Path::parent).unwrap_or(Path::new(""));
    normalize(&dir.join(include))
}

fn path_key(path: &Path) -> String {
    let s = normalize(path).to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

fn parse_uri(s: &str) -> Option<Uri> {
    s.parse().ok()
}

struct Doc {
    version: i32,
    an: Arc<Analysis>,
    let_types: Arc<HashMap<(usize, usize), String>>,
}

struct Server {
    send: Box<dyn Fn(Message)>,
    root: Option<PathBuf>,
    docs: HashMap<String, Doc>,
}

type Handler<P, R> = fn(&Server, P) -> Result<R, String>;

impl Server {
    fn new(send: impl Fn(Message) + 'static, root: Option<PathBuf>) -> Self {
        Server {
            send: Box::new(send),
            root,
            docs: HashMap::new(),
        }
    }

    fn handle_request(&mut self, req: Request) -> Response {
        let id = req.id.clone();
        let result = match req.method.as_str() {
            "textDocument/hover" => self.call(req, Server::hover),
            "textDocument/definition" => self.call(req, Server::definition),
            "textDocument/references" => self.call(req, Server::references),
            "textDocument/documentHighlight" => self.call(req, Server::highlight),
            "textDocument/documentSymbol" => self.call(req, Server::symbols),
            "textDocument/completion" => self.call(req, Server::completion),
            "textDocument/signatureHelp" => self.call(req, Server::signature_help),
            "textDocument/prepareRename" => self.call(req, Server::prepare_rename),
            "textDocument/rename" => self.call(req, Server::rename),
            "textDocument/inlayHint" => self.call(req, Server::inlay_hints),
            _ => {
                return Response::new_err(
                    id,
                    ErrorCode::MethodNotFound as i32,
                    format!("unsupported request {}", req.method),
                )
            }
        };
        match result {
            Ok(value) => Response::new_ok(id, value),
            Err(message) => Response::new_err(id, ErrorCode::RequestFailed as i32, message),
        }
    }

    fn call<P: DeserializeOwned, R: Serialize>(
        &self,
        req: Request,
        handler: Handler<P, R>,
    ) -> Result<Value, String> {
        let params: P = serde_json::from_value(req.params).map_err(|e| e.to_string())?;
        let result = handler(self, params)?;
        serde_json::to_value(result).map_err(|e| e.to_string())
    }

    fn handle_notification(&mut self, n: Notification) {
        match n.method.as_str() {
            "textDocument/didOpen" => {
                if let Ok(p) = serde_json::from_value::<DidOpenTextDocumentParams>(n.params) {
                    let doc = p.text_document;
                    self.update(doc.uri.as_str(), doc.text, doc.version);
                }
            }
            "textDocument/didChange" => {
                if let Ok(p) = serde_json::from_value::<DidChangeTextDocumentParams>(n.params) {
                    if let Some(change) = p.content_changes.into_iter().last() {
                        let uri = p.text_document.uri;
                        self.update(uri.as_str(), change.text, p.text_document.version);
                    }
                }
            }
            "textDocument/didSave" => self.publish_all(),
            "textDocument/didClose" => {
                if let Ok(p) = serde_json::from_value::<DidCloseTextDocumentParams>(n.params) {
                    let uri = p.text_document.uri;
                    self.docs.remove(uri.as_str());
                    self.send_diagnostics(uri, Vec::new(), None);
                }
            }
            _ => {}
        }
    }

    fn update(&mut self, uri: &str, text: String, version: i32) {
        self.docs.insert(
            uri.to_string(),
            Doc {
                version,
                an: Arc::new(Analysis::new(text)),
                let_types: Arc::default(),
            },
        );
        self.publish_all();
    }

    fn publish_all(&mut self) {
        let uris: Vec<String> = self.docs.keys().cloned().collect();
        for uri in uris {
            let path = uri_to_path(&uri);
            let an = self.docs[&uri].an.clone();
            let read = |p: &Path| self.read(p);
            let resolve = |from: Option<&Path>, import: &str| self.resolve_import(from, import);
            let loader = Loader {
                read: &read,
                resolve: &resolve,
                key: &path_key,
            };
            let result = check(&an, path.as_deref(), &loader);
            let doc = self.docs.get_mut(&uri).unwrap();
            doc.let_types = Arc::new(result.let_types);
            let version = doc.version;
            if let Some(uri) = parse_uri(&uri) {
                self.send_diagnostics(uri, result.diagnostics, Some(version));
            }
        }
    }

    fn send_diagnostics(&self, uri: Uri, diagnostics: Vec<Diagnostic>, version: Option<i32>) {
        let params = PublishDiagnosticsParams {
            uri,
            diagnostics,
            version,
        };
        (self.send)(Message::Notification(Notification::new(
            "textDocument/publishDiagnostics".into(),
            params,
        )));
    }

    fn open_doc_for(&self, path: &Path) -> Option<(&String, &Doc)> {
        let key = path_key(path);
        self.docs
            .iter()
            .find(|(uri, _)| uri_to_path(uri).is_some_and(|p| path_key(&p) == key))
    }

    fn read(&self, path: &Path) -> Option<String> {
        match self.open_doc_for(path) {
            Some((_, doc)) => Some(doc.an.text.clone()),
            None => fs::read_to_string(path).ok(),
        }
    }

    fn resolve_import(&self, from: Option<&Path>, import: &str) -> PathBuf {
        if let Some(name) = stdlib::name_of(import) {
            return stdlib::path(name);
        }
        let base = if import.starts_with("std/") {
            self.root.clone()
        } else {
            from.and_then(Path::parent).map(Path::to_path_buf)
        };
        normalize(&base.unwrap_or_default().join(import))
    }

    /// File named by the path literal `path` of the file `from`.
    fn literal_path(&self, from: Option<&Path>, kind: PathUse, path: &str) -> PathBuf {
        match kind {
            PathUse::Import => self.resolve_import(from, path),
            PathUse::IncludeBytes | PathUse::IncludeProgram => resolve_include(from, path),
        }
    }

    /// Hover text for the file behind an import or include path.
    fn describe_path(&self, kind: PathUse, path: &Path) -> String {
        let shown = self
            .root
            .as_deref()
            .and_then(|root| path.strip_prefix(root).ok())
            .unwrap_or(path);
        let what = match kind {
            PathUse::Import => "imported file",
            PathUse::IncludeBytes => "file embedded as bytes at compile time",
            PathUse::IncludeProgram => {
                "Psyco program, compiled for Linux x86-64 and embedded as an ELF executable"
            }
        };
        let mut text = format!("`{}`\n\n*{what}*", shown.to_string_lossy().replace('\\', "/"));
        if kind == PathUse::IncludeBytes {
            match fs::metadata(path) {
                Ok(m) if m.is_file() => text.push_str(&format!("\n\n{} bytes", m.len())),
                _ => text.push_str("\n\nFile not found."),
            }
        } else {
            match self.read(path) {
                Some(src) => {
                    if let Some(doc) = analysis::header_comment(&src) {
                        text.push_str(&format!("\n\n{doc}"));
                    }
                }
                None => text.push_str("\n\nFile not found."),
            }
        }
        text
    }

    /// Files and folders for the path being typed in `import "..."`,
    /// `include_bytes("...")` or `include_program("...")`. `typed` is the
    /// line up to the cursor; `None` outside of such a string.
    fn path_completion(
        &self,
        typed: &str,
        from: Option<&Path>,
        cursor: Position,
    ) -> Option<Vec<CompletionItem>> {
        let quote = typed.rfind('"')?;
        let (head, partial) = (typed[..quote].trim_end(), &typed[quote + 1..]);
        // An odd number of quotes before it: that quote closes a string.
        if head.matches('"').count() % 2 != 0 {
            return None;
        }
        let kind = PathUse::before_quote(head)?;
        let prefix = partial.rsplit('/').next().unwrap_or(partial);
        let dir = self.literal_path(from, kind, &partial[..partial.len() - prefix.len()]);
        let own = from.map(path_key);
        let start = Position::new(
            cursor.line,
            cursor.character.saturating_sub(prefix.encode_utf16().count() as u32),
        );

        let mut items = Vec::new();
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            let is_dir = path.is_dir();
            // Only sources can be imported or compiled; never the file itself.
            let usable = is_dir
                || kind == PathUse::IncludeBytes
                || path.extension().is_some_and(|e| e == "psy");
            if name.starts_with('.') || !usable || own == Some(path_key(&path)) {
                continue;
            }
            let text = if is_dir { format!("{name}/") } else { name };
            items.push(CompletionItem {
                label: text.clone(),
                kind: Some(if is_dir {
                    CompletionItemKind::FOLDER
                } else {
                    CompletionItemKind::FILE
                }),
                sort_text: Some(format!("{}{text}", if is_dir { 0 } else { 1 })),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                    Range::new(start, cursor),
                    text,
                ))),
                ..Default::default()
            });
        }
        Some(items)
    }

    fn world(&self, uri: &Uri) -> Result<World, String> {
        let uri = uri.as_str();
        let doc = self
            .docs
            .get(uri)
            .ok_or_else(|| format!("document {uri} is not open"))?;
        let mut units = vec![Unit {
            uri: uri.to_string(),
            path: uri_to_path(uri),
            an: doc.an.clone(),
            let_types: doc.let_types.clone(),
            alias: None,
        }];
        let mut seen: HashSet<String> = units[0].path.iter().map(|p| path_key(p)).collect();
        let mut next = 0;
        while next < units.len() {
            let from = units[next].path.clone();
            let an = units[next].an.clone();
            let imports: Vec<(String, Option<String>)> = an
                .imports
                .iter()
                .map(|(p, tok)| (p.clone(), an.import_alias(*tok).map(str::to_string)))
                .collect();
            next += 1;
            for (import, alias) in imports {
                let path = self.resolve_import(from.as_deref(), &import);
                if let Some(unit) = self.unit_for(path, alias, &mut seen) {
                    units.push(unit);
                }
            }
        }
        // The standard library is part of every program.
        for (name, _) in stdlib::FILES {
            if let Some(unit) = self.unit_for(stdlib::path(name), None, &mut seen) {
                units.push(unit);
            }
        }
        Ok(World { units })
    }

    /// The open document or the file at `path`, once per world.
    fn unit_for(&self, path: PathBuf, alias: Option<String>, seen: &mut HashSet<String>) -> Option<Unit> {
        if !seen.insert(path_key(&path)) {
            return None;
        }
        if let Some((uri, doc)) = self.open_doc_for(&path) {
            return Some(Unit {
                uri: uri.clone(),
                path: Some(path),
                an: doc.an.clone(),
                let_types: doc.let_types.clone(),
                alias,
            });
        }
        let text = match stdlib::FILES.iter().find(|(name, _)| stdlib::path(name) == path) {
            Some((name, _)) => stdlib::source(name, &|p| fs::read_to_string(p).ok()),
            None => fs::read_to_string(&path).ok()?,
        };
        Some(Unit {
            uri: path_to_uri(&path),
            path: Some(path),
            an: Arc::new(Analysis::new(text)),
            let_types: Arc::default(),
            alias,
        })
    }

    fn hover(&self, p: HoverParams) -> Result<Option<Hover>, String> {
        let pos = p.text_document_position_params;
        let world = self.world(&pos.text_document.uri)?;
        let an = &world.units[0].an;

        // Numbers and characters: the value in every base.
        if let Some(i) = an.literal_at(pos.position) {
            let (value, suffix) = match &an.tokens[i].kind {
                psycoc::TokenKind::IntSuffix(v, s) => (*v, Some(s.as_str())),
                psycoc::TokenKind::Int(v) => (*v, None),
                _ => unreachable!(),
            };
            let s = an.tokens[i].span;
            let is_char = an.line(s.line).chars().nth(s.col - 1) == Some('\'');
            let mut value = literal::describe(value, suffix, is_char);
            if let Some(note) = syscall_note(&world, i) {
                value.push_str(&format!("\n\n{note}"));
            }
            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value,
                }),
                range: Some(an.tok_range(i)),
            }));
        }

        // The file behind `import "..."`, `include_bytes("...")` or `include_program("...")`.
        let literal = an
            .string_at(pos.position)
            .and_then(|i| Some((i, an.path_literal(i)?)));
        if let Some((i, (kind, path))) = literal {
            let path = self.literal_path(world.units[0].path.as_deref(), kind, &path);
            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: self.describe_path(kind, &path),
                }),
                range: Some(an.tok_range(i)),
            }));
        }

        let Some(tok) = an.token_at(pos.position) else {
            return Ok(None);
        };
        let range = Some(an.tok_range(tok));
        let markdown = |value: String| {
            Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value,
                }),
                range,
            })
        };

        if an.is_attribute_name(tok) {
            let name = an.ident(tok).unwrap_or_default();
            return Ok(builtins::attribute(name).and_then(|a| {
                let usage = if a.target == "file" {
                    format!("#![{}]", a.name)
                } else {
                    format!("#[{}]", a.snippet.replace("${1:16}", "N"))
                };
                let on = if a.target == "file" {
                    "first line of a file".to_string()
                } else {
                    format!("`{}`", a.target)
                };
                markdown(format!(
                    "```psyco\n{usage}\n```\n\n*attribute on {on}*\n\n{}",
                    a.doc
                ))
            }));
        }
        if let Some(target) = world.resolve(0, tok).first() {
            let mut text = describe(&world, *target);
            if let Some(note) = syscall_note(&world, tok) {
                text.push_str(&format!("\n\n{note}"));
            }
            return Ok(markdown(text));
        }
        let word = an.ident(tok).map(str::to_string).unwrap_or_else(|| {
            let s = an.tokens[tok].span;
            an.line(s.line)
                .chars()
                .skip(s.col - 1)
                .take_while(|c| analysis::is_word(*c))
                .collect()
        });
        let builtin = if an.is_member(tok) {
            builtins::method(&word)
        } else {
            builtins::builtin(&word)
        };
        if let Some(b) = builtin {
            let mut text = format!("```psyco\n{}\n```\n\n{}", b.signature(), b.doc);
            if b.trusted {
                text.push_str("\n\nOnly available in `#![trusted]` files.");
            }
            return Ok(markdown(text));
        }
        // `as` of `import "file.psy" as m`, not a cast.
        if word == "as"
            && tok >= 2
            && matches!(an.kind(tok - 1), psycoc::TokenKind::Str(_))
            && *an.kind(tok - 2) == psycoc::TokenKind::Import
        {
            return Ok(markdown(
                "`as` (import): makes the file a module. Its items are reached as `m::item` and do not clash with names of other files."
                    .into(),
            ));
        }
        Ok(builtins::keyword_doc(&word).and_then(|doc| markdown(format!("`{word}`: {doc}"))))
    }

    fn definition(&self, p: GotoDefinitionParams) -> Result<Option<GotoDefinitionResponse>, String> {
        let pos = p.text_document_position_params;
        let world = self.world(&pos.text_document.uri)?;
        let an = &world.units[0].an;

        // `import "f.psy"`, `include_bytes("f")` and `include_program("f.psy")`
        // open the file: from the path under the cursor, or from anywhere on
        // the line (the first path there).
        let line = pos.position.line as usize + 1;
        let literal = an
            .string_at(pos.position)
            .and_then(|i| an.path_literal(i))
            .or_else(|| {
                (0..an.tokens.len())
                    .filter(|&i| an.tokens[i].span.line == line)
                    .find_map(|i| an.path_literal(i))
            });
        if let Some((kind, path)) = literal {
            let path = self.literal_path(world.units[0].path.as_deref(), kind, &path);
            let uri = self
                .open_doc_for(&path)
                .map(|(u, _)| u.clone())
                .or_else(|| path.exists().then(|| path_to_uri(&path)));
            if let Some(uri) = uri.as_deref().and_then(parse_uri) {
                return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                    uri,
                    range: Range::default(),
                })));
            }
        }

        let Some(tok) = an.token_at(pos.position) else {
            return Ok(None);
        };
        let locations: Vec<Location> = world
            .resolve(0, tok)
            .into_iter()
            .filter_map(|t| match imported_unit(&world, t) {
                // An `import ... as m` alias: the file itself.
                Some(u) => Some(Location {
                    uri: parse_uri(&world.units[u].uri)?,
                    range: Range::default(),
                }),
                None => location(&world, t.unit, world.def(t).tok),
            })
            .collect();
        Ok((!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations)))
    }

    fn references(&self, p: ReferenceParams) -> Result<Option<Vec<Location>>, String> {
        let pos = p.text_document_position;
        let world = self.world(&pos.text_document.uri)?;
        let Some(target) = target_at(&world, pos.position) else {
            return Ok(None);
        };
        let def_tok = world.def(target).tok;
        let refs = world
            .references(target, 0..world.units.len())
            .into_iter()
            .filter(|&(unit, tok)| {
                p.context.include_declaration || !(unit == target.unit && tok == def_tok)
            })
            .filter_map(|(unit, tok)| location(&world, unit, tok))
            .collect();
        Ok(Some(refs))
    }

    fn highlight(&self, p: DocumentHighlightParams) -> Result<Option<Vec<DocumentHighlight>>, String> {
        let pos = p.text_document_position_params;
        let world = self.world(&pos.text_document.uri)?;
        let Some(target) = target_at(&world, pos.position) else {
            return Ok(None);
        };
        let an = &world.units[0].an;
        let highlights = world
            .references(target, std::iter::once(0))
            .into_iter()
            .map(|(_, tok)| DocumentHighlight {
                range: an.tok_range(tok),
                kind: Some(if target.unit == 0 && world.def(target).tok == tok {
                    DocumentHighlightKind::WRITE
                } else {
                    DocumentHighlightKind::READ
                }),
            })
            .collect();
        Ok(Some(highlights))
    }

    fn symbols(&self, p: DocumentSymbolParams) -> Result<Option<DocumentSymbolResponse>, String> {
        let doc = self
            .docs
            .get(p.text_document.uri.as_str())
            .ok_or("document is not open")?;
        let an = &doc.an;
        let symbol = |d: &Def, children: Option<Vec<DocumentSymbol>>| {
            let selection_range = an.tok_range(d.tok);
            let start = an.tok_range(d.start_tok.min(d.tok)).start;
            let end = an.tok_range(d.end_tok.max(d.tok)).end;
            #[allow(deprecated)]
            DocumentSymbol {
                name: d.qualified_name(),
                detail: Some(d.detail.clone()),
                kind: symbol_kind(d.kind),
                tags: None,
                deprecated: None,
                range: Range::new(start, end),
                selection_range,
                children,
            }
        };
        let symbols = an
            .defs
            .iter()
            .filter(|d| d.kind.is_item() || d.kind == DefKind::Method)
            .filter(|d| !(d.kind == DefKind::Function && d.container.is_some()))
            .map(|d| {
                let children: Vec<DocumentSymbol> = an
                    .defs
                    .iter()
                    .filter(|c| {
                        matches!(d.kind, DefKind::Struct | DefKind::Enum)
                            && matches!(c.kind, DefKind::Field | DefKind::Variant)
                            && c.container.as_deref() == Some(d.name.as_str())
                            && d.start_tok <= c.tok
                            && c.tok <= d.end_tok
                    })
                    .map(|c| symbol(c, None))
                    .collect();
                symbol(d, (!children.is_empty()).then_some(children))
            })
            .collect::<Vec<_>>();
        let mut symbols = symbols;
        symbols.sort_by_key(|s| (s.range.start.line, s.range.start.character));
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    fn completion(&self, p: CompletionParams) -> Result<Option<CompletionResponse>, String> {
        let pos = p.text_document_position;
        let world = self.world(&pos.text_document.uri)?;
        let an = &world.units[0].an;
        let (line, col) = an.line_col(pos.position);
        let chars: Vec<char> = an.line(line).chars().collect();
        let cursor = (col - 1).min(chars.len());
        let mut start = cursor;
        while start > 0 && analysis::is_word(chars[start - 1]) {
            start -= 1;
        }
        let before: String = chars[..start].iter().collect();
        let before = before.trim_end();

        let typed: String = chars[..cursor].iter().collect();
        let from = world.units[0].path.as_deref();
        if let Some(items) = self.path_completion(&typed, from, pos.position) {
            return Ok(Some(CompletionResponse::Array(items)));
        }

        let mut items = Vec::new();
        let mut seen = HashSet::new();
        let mut add = |item: CompletionItem| {
            if seen.insert((item.label.clone(), format!("{:?}", item.kind))) {
                items.push(item);
            }
        };
        let def_item = |d: &Def, label: String| CompletionItem {
            label,
            kind: Some(completion_kind(d.kind)),
            detail: Some(d.detail.clone()),
            ..Default::default()
        };

        // Inside `#[...` or `#![...`, possibly after other attributes and commas.
        let open_attr = before.rfind("#[").or_else(|| before.rfind("#!["));
        let in_attr = open_attr.is_some_and(|i| {
            let inside = &before[i..];
            !inside.contains(']') && (inside.ends_with('[') || inside.ends_with(','))
        });
        let file_attr = in_attr && before[open_attr.unwrap_or(0)..].starts_with("#![");
        if in_attr {
            for a in builtins::ATTRIBUTES {
                if (a.target == "file") != file_attr {
                    continue;
                }
                add(CompletionItem {
                    label: a.name.into(),
                    kind: Some(CompletionItemKind::PROPERTY),
                    detail: Some(format!("attribute on {}", a.target)),
                    documentation: Some(Documentation::String(a.doc.into())),
                    insert_text: Some(a.snippet.into()),
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    ..Default::default()
                });
            }
        } else if let Some(path) = before.strip_suffix("::") {
            let owner: String = path
                .chars()
                .rev()
                .take_while(|c| analysis::is_word(*c))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            // Variants, methods, items of a module or of a file imported `as`.
            let at = an.cursor_index(line, col);
            for t in world.resolve_path(0, at, &owner, None) {
                let d = world.def(t);
                add(def_item(d, d.name.clone()));
            }
        } else if before.ends_with('.') && !before.ends_with("..") {
            for t in world.find_all(|d| matches!(d.kind, DefKind::Field | DefKind::Method)) {
                let d = world.def(t);
                let mut item = def_item(d, d.name.clone());
                item.label_details = d.container.as_ref().map(|c| CompletionItemLabelDetails {
                    detail: None,
                    description: Some(c.clone()),
                });
                add(item);
            }
            for b in builtins::METHODS {
                add(CompletionItem {
                    label: b.name.into(),
                    kind: Some(CompletionItemKind::METHOD),
                    detail: Some(b.signature()),
                    documentation: Some(Documentation::String(b.doc.into())),
                    ..Default::default()
                });
            }
        } else {
            let cursor_tok = an.cursor_index(line, col);
            // Snippets only where a statement or an item can start.
            let starts_statement = before.is_empty()
                || ["{", "}", ";", "=>"].iter().any(|t| before.ends_with(t));
            let mut snippet_labels = HashSet::new();
            if starts_statement {
                let top_level = an.brace_depth(cursor_tok) == 0;
                for (label, description, body) in builtins::SNIPPETS {
                    let item_snippet = matches!(
                        *label,
                        "fn" | "fn ->" | "main" | "struct" | "enum" | "impl" | "impl for" | "trait"
                            | "mod" | "const" | "static" | "static mut" | "import" | "import as"
                    );
                    let in_body_too = matches!(*label, "fn" | "fn ->");
                    if item_snippet != top_level && !in_body_too {
                        continue;
                    }
                    snippet_labels.insert(*label);
                    add(CompletionItem {
                        label: (*label).into(),
                        kind: Some(CompletionItemKind::SNIPPET),
                        detail: Some((*description).into()),
                        insert_text: Some((*body).into()),
                        insert_text_format: Some(InsertTextFormat::SNIPPET),
                        ..Default::default()
                    });
                }
            }
            let at = cursor_tok.saturating_sub(1);
            for d in an.locals_at(at) {
                add(def_item(d, d.name.clone()));
            }
            for t in world.find_all(|d| d.kind.is_item()) {
                add(def_item(world.def(t), world.def(t).name.clone()));
            }
            for t in world.prelude_variants(None) {
                add(def_item(world.def(t), world.def(t).name.clone()));
            }
            for b in builtins::BUILTINS {
                add(CompletionItem {
                    label: b.name.into(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: Some(b.signature()),
                    documentation: Some(Documentation::String(b.doc.into())),
                    ..Default::default()
                });
            }
            for (word, doc) in builtins::PRIMITIVES {
                add(CompletionItem {
                    label: (*word).into(),
                    kind: Some(CompletionItemKind::TYPE_PARAMETER),
                    detail: Some((*doc).into()),
                    ..Default::default()
                });
            }
            for (word, doc) in builtins::KEYWORDS {
                if snippet_labels.contains(word) {
                    continue;
                }
                add(CompletionItem {
                    label: (*word).into(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    detail: Some((*doc).into()),
                    ..Default::default()
                });
            }
        }
        Ok(Some(CompletionResponse::Array(items)))
    }

    fn signature_help(&self, p: SignatureHelpParams) -> Result<Option<SignatureHelp>, String> {
        let pos = p.text_document_position_params;
        let world = self.world(&pos.text_document.uri)?;
        let an = &world.units[0].an;
        let (line, col) = an.line_col(pos.position);
        let Some((open, mut active)) = an.call_at(an.cursor_index(line, col)) else {
            return Ok(None);
        };
        let Some(callee) = open.checked_sub(1).filter(|&t| an.ident(t).is_some()) else {
            return Ok(None);
        };

        let (label, params, doc) = match world
            .resolve(0, callee)
            .into_iter()
            .map(|t| world.def(t))
            .find(|d| matches!(d.kind, DefKind::Function | DefKind::Method))
        {
            Some(d) => {
                let mut params = d.params.clone();
                if an.is_member(callee) && params.first().is_some_and(|p| p.contains("self")) {
                    params.remove(0);
                    let label = d.detail.replacen(&d.params[0], "", 1);
                    let label = label.replacen("(, ", "(", 1);
                    (label, params, None)
                } else {
                    (d.detail.clone(), params, None)
                }
            }
            None => match if an.is_member(callee) {
                builtins::method(an.ident(callee).unwrap_or_default())
            } else {
                builtins::builtin(an.ident(callee).unwrap_or_default())
            } {
                Some(b) => {
                    // `syscall(60, ...)`: which call it is, to know its arguments.
                    let note = syscall_note(&world, open + 1).filter(|_| b.name == "syscall");
                    let doc = note.map_or_else(|| b.doc.to_string(), |n| format!("{}\n\n{n}", b.doc));
                    (
                        b.signature(),
                        b.params.iter().map(|s| s.to_string()).collect(),
                        Some(doc),
                    )
                }
                None => return Ok(None),
            },
        };
        if params.len() == 1 && params[0] == "values..." {
            active = 0;
        }
        Ok(Some(SignatureHelp {
            signatures: vec![SignatureInformation {
                label,
                documentation: doc.map(Documentation::String),
                parameters: Some(
                    params
                        .into_iter()
                        .map(|p| ParameterInformation {
                            label: ParameterLabel::Simple(p),
                            documentation: None,
                        })
                        .collect(),
                ),
                active_parameter: Some(active),
            }],
            active_signature: Some(0),
            active_parameter: Some(active),
        }))
    }

    fn rename_target(&self, uri: &Uri, position: Position) -> Result<(World, Target), String> {
        let world = self.world(uri)?;
        let target = target_at(&world, position).ok_or("nothing to rename here")?;
        if !world.def(target).kind.is_local() {
            return Err("only local variables and parameters can be renamed".into());
        }
        Ok((world, target))
    }

    fn prepare_rename(&self, p: TextDocumentPositionParams) -> Result<Option<PrepareRenameResponse>, String> {
        let (world, _) = self.rename_target(&p.text_document.uri, p.position)?;
        let an = &world.units[0].an;
        Ok(an
            .token_at(p.position)
            .map(|tok| PrepareRenameResponse::Range(an.tok_range(tok))))
    }

    fn rename(&self, p: RenameParams) -> Result<Option<WorkspaceEdit>, String> {
        let pos = p.text_document_position;
        let name = p.new_name;
        let valid = name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && name.chars().all(analysis::is_word)
            && !builtins::KEYWORDS.iter().any(|(k, _)| *k == name);
        if !valid {
            return Err(format!("'{name}' is not a valid identifier"));
        }
        let (world, target) = self.rename_target(&pos.text_document.uri, pos.position)?;
        let an = &world.units[target.unit].an;
        let edits = world
            .references(target, std::iter::once(target.unit))
            .into_iter()
            .map(|(_, tok)| TextEdit::new(an.tok_range(tok), name.clone()))
            .collect();
        let uri = parse_uri(&world.units[target.unit].uri).ok_or("invalid document URI")?;
        Ok(Some(WorkspaceEdit {
            changes: Some(HashMap::from([(uri, edits)])),
            ..Default::default()
        }))
    }

    fn inlay_hints(&self, p: InlayHintParams) -> Result<Option<Vec<InlayHint>>, String> {
        let doc = self
            .docs
            .get(p.text_document.uri.as_str())
            .ok_or("document is not open")?;
        let an = &doc.an;
        let hints = an
            .defs
            .iter()
            .filter(|d| d.kind == DefKind::Local && !d.explicit_type)
            .filter_map(|d| {
                let s = an.tokens.get(d.start_tok)?.span;
                let ty = doc.let_types.get(&(s.line, s.col))?;
                let position = an.tok_range(d.tok).end;
                (p.range.start <= position && position <= p.range.end).then(|| InlayHint {
                    position,
                    label: InlayHintLabel::String(format!(": {ty}")),
                    kind: Some(InlayHintKind::TYPE),
                    text_edits: None,
                    tooltip: None,
                    padding_left: None,
                    padding_right: None,
                    data: None,
                })
            })
            .collect();
        Ok(Some(hints))
    }
}

fn target_at(world: &World, position: Position) -> Option<Target> {
    let tok = world.units[0].an.token_at(position)?;
    world.resolve(0, tok).into_iter().next()
}

fn location(world: &World, unit: usize, tok: usize) -> Option<Location> {
    let u = &world.units[unit];
    Some(Location {
        uri: parse_uri(&u.uri)?,
        range: u.an.tok_range(tok),
    })
}

/// Names the Linux system call when token `tok` of the main unit is the
/// number given to `syscall(...)`: a literal, or a constant with a literal value.
fn syscall_note(world: &World, tok: usize) -> Option<String> {
    let an = &world.units[0].an;
    if !an.is_syscall_number(tok) {
        return None;
    }
    let number = match an.kind(tok) {
        psycoc::TokenKind::Int(v) | psycoc::TokenKind::IntSuffix(v, _) => *v,
        _ => {
            let target = *world.resolve(0, tok).first()?;
            world.units[target.unit].an.const_int(world.def(target))?
        }
    };
    let call = builtins::linux_syscall(number)?;
    Some(format!("Linux x86-64 system call {number}: `{call}`"))
}

/// The unit of the file an `import "..." as m` alias names.
fn imported_unit(world: &World, target: Target) -> Option<usize> {
    let d = world.def(target);
    if d.kind != DefKind::Module || !d.detail.starts_with("import ") {
        return None;
    }
    world
        .units
        .iter()
        .position(|u| u.alias.as_deref() == Some(d.name.as_str()))
}

fn describe(world: &World, target: Target) -> String {
    let unit = &world.units[target.unit];
    let d = world.def(target);
    let start = unit.an.kind(d.start_tok);
    let loop_var = d.kind == DefKind::Local && *start == psycoc::TokenKind::For;
    // A local that no `let` nor `for` introduces comes from a pattern.
    let binding = d.kind == DefKind::Local
        && !matches!(start, psycoc::TokenKind::For | psycoc::TokenKind::Let);
    let mut code = if loop_var { d.name.clone() } else { d.detail.clone() };
    if d.kind == DefKind::Local && !d.explicit_type {
        let s = unit.an.tokens[d.start_tok].span;
        if let Some(ty) = unit.let_types.get(&(s.line, s.col)) {
            code = format!("{code}: {ty}");
        }
    }
    let mut text = format!("```psyco\n{code}\n```");
    let owner = d.container.as_deref().unwrap_or_default();
    let note = match d.kind {
        DefKind::Field => format!("field of `{owner}`"),
        DefKind::Variant => format!("variant of `{owner}`"),
        DefKind::Method => {
            let is_trait = !world
                .find_all(|t| t.kind == DefKind::Trait && t.name == owner)
                .is_empty();
            match unit.an.impl_at(d.start_tok).and_then(|b| b.trait_name.as_deref()) {
                Some(tr) => format!("method of `{owner}`, from `impl {tr} for {owner}`"),
                None if is_trait => format!("method of trait `{owner}`"),
                None => format!("method of `{owner}`"),
            }
        }
        DefKind::TypeParam => format!("type parameter of `{owner}`"),
        DefKind::Module if imported_unit(world, target).is_some() => "file imported as a module".into(),
        DefKind::Module => "module".into(),
        DefKind::Local if binding => "bound by a pattern".into(),
        DefKind::Param => "parameter".into(),
        DefKind::Function if d.container.is_some() => {
            let attr = if d.name.starts_with("set_") { "setter" } else { "getter" };
            format!("generated by `#[{attr}]` on `static {owner}`")
        }
        DefKind::Function => {
            let attrs = unit.an.attributes_before(d.start_tok);
            let entry = |attr: &str| attrs.iter().any(|a| a == attr);
            if entry("syscall") {
                "system call entry point (`#[syscall]`), cannot be called directly".into()
            } else if entry("interrupt") {
                "interrupt handler (`#[interrupt]`), cannot be called directly".into()
            } else {
                String::new()
            }
        }
        DefKind::Local if loop_var => format!("loop variable of `{} {{ }}`", d.detail),
        _ => String::new(),
    };
    if !note.is_empty() {
        text.push_str(&format!("\n\n*{note}*"));
    }
    if let Some(m) = d.module.as_ref().filter(|_| d.kind != DefKind::Module) {
        text.push_str(&format!("\n\nIn `mod {m}`"));
    }
    if let Some(u) = imported_unit(world, target) {
        // An alias: what the imported file says about itself.
        if let Some(doc) = analysis::header_comment(&world.units[u].an.text) {
            text.push_str(&format!("\n\n{doc}"));
        }
    } else if let Some(doc) = unit.an.doc_comment(d) {
        text.push_str(&format!("\n\n{doc}"));
    }
    if target.unit != 0 {
        if let Some(name) = unit.path.as_ref().and_then(|p| p.file_name()) {
            text.push_str(&format!("\n\nDefined in `{}`", name.to_string_lossy()));
        }
    }
    text
}

fn symbol_kind(kind: DefKind) -> SymbolKind {
    match kind {
        DefKind::Function => SymbolKind::FUNCTION,
        DefKind::Method => SymbolKind::METHOD,
        DefKind::Struct => SymbolKind::STRUCT,
        DefKind::Field => SymbolKind::FIELD,
        DefKind::Enum => SymbolKind::ENUM,
        DefKind::Variant => SymbolKind::ENUM_MEMBER,
        DefKind::Const => SymbolKind::CONSTANT,
        DefKind::Static => SymbolKind::VARIABLE,
        DefKind::Local | DefKind::Param => SymbolKind::VARIABLE,
        DefKind::Trait => SymbolKind::INTERFACE,
        DefKind::Module => SymbolKind::MODULE,
        DefKind::TypeParam => SymbolKind::TYPE_PARAMETER,
    }
}

fn completion_kind(kind: DefKind) -> CompletionItemKind {
    match kind {
        DefKind::Function => CompletionItemKind::FUNCTION,
        DefKind::Method => CompletionItemKind::METHOD,
        DefKind::Struct => CompletionItemKind::STRUCT,
        DefKind::Field => CompletionItemKind::FIELD,
        DefKind::Enum => CompletionItemKind::ENUM,
        DefKind::Variant => CompletionItemKind::ENUM_MEMBER,
        DefKind::Const => CompletionItemKind::CONSTANT,
        DefKind::Static => CompletionItemKind::VARIABLE,
        DefKind::Local | DefKind::Param => CompletionItemKind::VARIABLE,
        DefKind::Trait => CompletionItemKind::INTERFACE,
        DefKind::Module => CompletionItemKind::MODULE,
        DefKind::TypeParam => CompletionItemKind::TYPE_PARAMETER,
    }
}
