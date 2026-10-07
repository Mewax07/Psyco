(function_item
  "fn" @context
  name: (_) @name) @item

(struct_item
  "struct" @context
  name: (_) @name) @item

(field_declaration
  name: (_) @name) @item

(enum_item
  "enum" @context
  name: (_) @name) @item

(enum_variant
  name: (_) @name) @item

(impl_item
  "impl" @context
  type: (_) @name) @item

(const_item
  "const" @context
  name: (_) @name) @item

(static_item
  "static" @context
  name: (_) @name) @item
