(function_declaration
  "fn" @context
  receiver: (identifier)? @context
  name: (identifier) @name) @item

(type_declaration
  kind: _ @context
  name: (identifier) @name) @item

(test_declaration
  keyword: (identifier) @context
  name: (string) @name) @item
