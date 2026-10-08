# Psyco for Zed

Syntax highlighting (tree-sitter) and a language server (`psyco-lsp`) for `.psy` files.

## Language server

`psyco-lsp` reuses the `psycoc` front-end, so its diagnostics are the compiler's own
lexer, parser and type errors, imported files included. It also provides:

- hover: signatures, `//` doc comments, the type of variables (`let`, `for` and parameters),
  every base for numbers and characters (`0xFFu8` -> dec, hex, bin, oct, signed, char),
  builtins, built-in methods (`len`, `as_ptr`, `add`...) and attributes
- go to definition (including into imported files and `import "..."` paths)
- find references and document highlights
- document outline (functions, methods, structs and fields, enums and variants, consts, statics)
- completion: locals, items, `Enum::`/`Type::` members, fields and built-in methods after `.`,
  builtins, keywords, attributes after `#[` / `#![` (also inside lists like `#[getter, setter]`)
- `#[getter]` / `#[setter]` statics: the generated `get_NAME()` / `set_NAME(value)` have hover,
  completion, signature help and go to definition (to the static)
- snippets: `if`, `if else`, `else if`, `while`, `for`, `loop`, `match`, `let`, `let mut`
  in function bodies; `fn`, `main`, `struct`, `enum`, `impl`, `const`, `static`, `import` at the top level
- signature help
- inlay type hints for `let` and `for` variables
- diagnostics for every function with a type error, not only the first one
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
