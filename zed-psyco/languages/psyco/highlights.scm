; Zed: when several patterns match a node, the last one wins.
; Generic rules first, specific ones after.

(identifier) @variable

; Identifiers that look like constants or types.
((identifier) @constant
  (#match? @constant "^[A-Z][A-Z0-9_]+$"))

((identifier) @type
  (#match? @type "^[A-Z][a-z0-9]+[A-Za-z0-9]*$"))

; ---- types --------------------------------------------------------------

(type_identifier) @type
(primitive_type) @type.builtin
(unit_type) @type.builtin

; ---- definitions --------------------------------------------------------

(function_item name: (identifier) @function)

(parameter pattern: (identifier) @variable.parameter)
(self) @variable.special
(wildcard_pattern) @variable.special

(let_declaration pattern: (identifier) @variable)
(for_statement pattern: (identifier) @variable)

(field_identifier) @property

(const_item name: (identifier) @constant)
(static_item name: (identifier) @constant)

(enum_variant name: (identifier) @constant)

; ---- uses ---------------------------------------------------------------

; `Enum::Variant` / `Type::method`
(scoped_identifier path: (type_identifier) @type)
(scoped_identifier name: (identifier) @constant)

(call_expression
  function: (identifier) @function)

(call_expression
  function: (scoped_identifier name: (identifier) @function))

(call_expression
  function: (field_expression field: (field_identifier) @function.method))

(call_expression
  function: (identifier) @function.builtin
  (#any-of? @function.builtin
    "print" "panic" "exit"
    "outb" "outw" "outl" "inb" "inw" "inl"
    "cli" "sti" "hlt" "pause" "int3" "ud2"
    "read_cs" "read_ds" "read_ss" "read_tr"
    "read_cr0" "read_cr2" "read_cr3" "read_cr4" "rdtsc"
    "write_cr0" "write_cr3" "write_cr4"
    "rdmsr" "wrmsr" "invlpg" "load_cs" "load_ds" "ltr"
    "memcpy" "memset" "efi_image_handle" "efi_system_table"
    "switch_stack" "lgdt" "lidt"))

; Methods built into arrays, slices, strings and raw pointers.
(call_expression
  function: (field_expression field: (field_identifier) @function.builtin)
  (#any-of? @function.builtin
    "len" "as_ptr" "as_mut_ptr" "is_null" "add" "sub" "offset" "read" "write"))

; ---- attributes ---------------------------------------------------------

(attribute) @attribute
(attribute_item name: (identifier) @attribute)
(attribute_item argument: (integer_literal) @number)
(inner_attribute) @attribute
(inner_attribute (identifier) @attribute)

; ---- literals -----------------------------------------------------------

(integer_literal) @number
(negative_literal) @number
(string_literal) @string
(char_literal) @string
(escape_sequence) @string.escape
(boolean_literal) @boolean

(line_comment) @comment
(block_comment) @comment

((line_comment) @comment.doc
  (#match? @comment.doc "^///"))

; ---- keywords -----------------------------------------------------------

[
  "fn"
  "struct"
  "enum"
  "impl"
  "const"
  "static"
  "extern"
  "let"
  "import"
] @keyword

[
  "if"
  "else"
  "while"
  "loop"
  "for"
  "in"
  "match"
  "return"
  (break_statement)
  (continue_statement)
] @keyword

[
  "as"
  "sizeof"
] @keyword

(mutable_specifier) @keyword

; ---- operators and punctuation ------------------------------------------

[
  "+" "-" "*" "/" "%" "+%" "-%" "*%"
  "&" "|" "^" "~" "!" "<<" ">>"
  "==" "!=" "<" "<=" ">" ">="
  "&&" "||"
  "=" "+=" "-=" "*=" "/=" "%=" "&=" "|=" "^=" "<<=" ">>="
  ".." "->" "=>"
] @operator

["(" ")" "[" "]" "{" "}"] @punctuation.bracket

["," ";" ":" "::" "."] @punctuation.delimiter

"#" @punctuation.special
