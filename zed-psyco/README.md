# Psyco for Zed

Syntax highlighting (tree-sitter) and a language server (`psyco-lsp`) for `.psy` files.

## Language server

`psyco-lsp` reuses the `psycoc` front-end, so its diagnostics are the compiler's own
lexer, parser and type errors, imported files included. It also provides:

- hover (signatures, `//` doc comments, inferred types of `let` bindings, builtins)
- go to definition (including into imported files and `import "..."` paths)
- find references and document highlights
- document outline (functions, methods, structs and fields, enums and variants, consts, statics)
- completion (locals, items, `Enum::`/`Type::` members, fields after `.`, builtins, keywords)
- signature help
- inlay type hints for `let` without an annotation
- renaming local variables and parameters

### Install

```sh
cargo install --path zed-psyco/psyco-lsp
```

This puts `psyco-lsp` in `~/.cargo/bin`, which must be on your `PATH`. Then in Zed:
**Extensions → Install Dev Extension** and pick the `zed-psyco` folder.

To use another binary, add this to your Zed `settings.json`:

```json
{
  "lsp": {
    "psyco-lsp": {
      "binary": { "path": "D:/path/to/psyco-lsp.exe" }
    }
  }
}
```

Inlay hints are off by default in Zed: turn them on with `"inlay_hints": { "enabled": true }`.
