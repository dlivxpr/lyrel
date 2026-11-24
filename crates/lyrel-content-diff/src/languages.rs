//! Per-language structural specifications: which top-level tree-sitter nodes
//! are reviewable items, and how each one's identity name is read.
//!
//! This module is the #390 extension point. Adding a format means adding one
//! grammar dependency to the crate manifest and one [`LangSpec`] entry here —
//! nothing in the CLI, protocol, or production desktop dependency graph
//! changes shape.

use tree_sitter::Language;

/// How a structural item's identity name is read off its node.
pub(crate) enum NameRule {
    /// The text of the first present node field — an identifier, key, or
    /// declaration name.
    Field(&'static [&'static str]),
    /// The first line of the node — a TOML table's `[section]` header, as
    /// opposed to the keys inside it.
    FirstLine,
    /// The node's header: its text up to the start of its `body` field (or
    /// the whole text when there is no body). What an `impl Foo for Bar`
    /// answers to, as opposed to the hundreds of lines inside it.
    Header,
    /// The name of the node's only `variable_declarator`. Several declarators
    /// in one statement (`const a = 1, b = 2`) have no single identity, so
    /// they stay anonymous.
    Declarator,
    /// Anonymous: identity is the full normalized text.
    Text,
}

pub(crate) struct KindSpec {
    /// The tree-sitter node kind, e.g. `function_item`.
    pub(crate) kind: &'static str,
    /// The UI label slug for the item family, e.g. `function`.
    pub(crate) label: &'static str,
    pub(crate) name: NameRule,
}

pub(crate) struct LangSpec {
    pub(crate) name: &'static str,
    pub(crate) extensions: &'static [&'static str],
    pub(crate) language: fn() -> Language,
    pub(crate) root: &'static str,
    pub(crate) kinds: &'static [KindSpec],
    /// Wrapper node kinds whose inner declaration supplies the item's
    /// identity; the wrapper's own text and span stay part of the block.
    pub(crate) unwrap: fn(&str) -> bool,
    /// Indentation carries structure (Python, YAML): a change in a line's
    /// leading whitespace is a real edit, never formatting only.
    pub(crate) significant_indent: bool,
}

const NO_UNWRAP: fn(&str) -> bool = |_| false;

/// `export function f`, `export interface T`, `export const x = 1`: the
/// wrapper stays part of the item's text and span, and classification looks
/// through it to the declaration.
fn export_unwrap(kind: &str) -> bool {
    kind == "export_statement"
}

/// Python decorators are part of the definition they annotate.
fn python_unwrap(kind: &str) -> bool {
    kind == "decorated_definition"
}

/// TypeScript adds ambient (`declare …`) wrappers around signatures and
/// modules.
fn ts_unwrap(kind: &str) -> bool {
    matches!(kind, "export_statement" | "ambient_declaration")
}

const RUST_KINDS: &[KindSpec] = &[
    KindSpec {
        kind: "function_item",
        label: "function",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "struct_item",
        label: "struct",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "union_item",
        label: "struct",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "enum_item",
        label: "enum",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "trait_item",
        label: "trait",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "impl_item",
        label: "impl",
        name: NameRule::Header,
    },
    KindSpec {
        kind: "mod_item",
        label: "module",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "const_item",
        label: "constant",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "static_item",
        label: "constant",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "type_item",
        label: "type",
        name: NameRule::Field(&["type_identifier", "name"]),
    },
    KindSpec {
        kind: "macro_definition",
        label: "macro",
        name: NameRule::Field(&["name"]),
    },
];

const PYTHON_KINDS: &[KindSpec] = &[
    KindSpec {
        kind: "function_definition",
        label: "function",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "class_definition",
        label: "class",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "import_statement",
        label: "import",
        name: NameRule::Text,
    },
    KindSpec {
        kind: "import_from_statement",
        label: "import",
        name: NameRule::Text,
    },
];

const GO_KINDS: &[KindSpec] = &[
    KindSpec {
        kind: "function_declaration",
        label: "function",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "method_declaration",
        label: "method",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "type_declaration",
        label: "type",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "type_spec",
        label: "type",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "type_alias",
        label: "type",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "const_declaration",
        label: "constant",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "const_spec",
        label: "constant",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "var_declaration",
        label: "variable",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "var_spec",
        label: "variable",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "import_declaration",
        label: "import",
        name: NameRule::Text,
    },
];

const JS_KINDS: &[KindSpec] = &[
    KindSpec {
        kind: "function_declaration",
        label: "function",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "generator_function_declaration",
        label: "function",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "class_declaration",
        label: "class",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "lexical_declaration",
        label: "variable",
        name: NameRule::Declarator,
    },
    KindSpec {
        kind: "variable_declaration",
        label: "variable",
        name: NameRule::Declarator,
    },
    KindSpec {
        kind: "import_statement",
        label: "import",
        name: NameRule::Text,
    },
];

/// TypeScript and TSX share these declarations; the two grammars differ only
/// in whether JSX is part of the expression grammar.
const TS_KINDS: &[KindSpec] = &[
    KindSpec {
        kind: "function_declaration",
        label: "function",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "generator_function_declaration",
        label: "function",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "function_signature",
        label: "function",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "class_declaration",
        label: "class",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "abstract_class_declaration",
        label: "class",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "interface_declaration",
        label: "type",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "type_alias_declaration",
        label: "type",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "enum_declaration",
        label: "enum",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "module",
        label: "module",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "internal_module",
        label: "module",
        name: NameRule::Field(&["name"]),
    },
    KindSpec {
        kind: "lexical_declaration",
        label: "variable",
        name: NameRule::Declarator,
    },
    KindSpec {
        kind: "variable_declaration",
        label: "variable",
        name: NameRule::Declarator,
    },
    KindSpec {
        kind: "import_statement",
        label: "import",
        name: NameRule::Text,
    },
];

const JSON_KINDS: &[KindSpec] = &[
    KindSpec {
        kind: "pair",
        label: "key",
        name: NameRule::Field(&["key"]),
    },
    KindSpec {
        kind: "array",
        label: "array",
        name: NameRule::Text,
    },
];

const TOML_KINDS: &[KindSpec] = &[
    KindSpec {
        kind: "pair",
        label: "key",
        name: NameRule::Field(&["key"]),
    },
    KindSpec {
        kind: "table",
        label: "section",
        name: NameRule::FirstLine,
    },
    KindSpec {
        kind: "table_array_element",
        label: "section",
        name: NameRule::FirstLine,
    },
    KindSpec {
        kind: "comment",
        label: "comment",
        name: NameRule::Text,
    },
];

const YAML_KINDS: &[KindSpec] = &[
    KindSpec {
        kind: "block_mapping_pair",
        label: "key",
        name: NameRule::Field(&["key"]),
    },
    KindSpec {
        kind: "block_sequence",
        label: "sequence",
        name: NameRule::Text,
    },
    KindSpec {
        kind: "comment",
        label: "comment",
        name: NameRule::Text,
    },
];

pub(crate) const SPECS: &[LangSpec] = &[
    LangSpec {
        name: "rust",
        extensions: &["rs"],
        language: || tree_sitter_rust::LANGUAGE.into(),
        root: "source_file",
        kinds: RUST_KINDS,
        unwrap: NO_UNWRAP,
        significant_indent: false,
    },
    LangSpec {
        name: "javascript",
        extensions: &["js", "mjs", "cjs", "jsx"],
        language: || tree_sitter_javascript::LANGUAGE.into(),
        root: "program",
        kinds: JS_KINDS,
        unwrap: export_unwrap,
        significant_indent: false,
    },
    LangSpec {
        name: "typescript",
        extensions: &["ts", "mts", "cts"],
        language: || tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        root: "program",
        kinds: TS_KINDS,
        unwrap: ts_unwrap,
        significant_indent: false,
    },
    LangSpec {
        name: "tsx",
        extensions: &["tsx"],
        language: || tree_sitter_typescript::LANGUAGE_TSX.into(),
        root: "program",
        kinds: TS_KINDS,
        unwrap: ts_unwrap,
        significant_indent: false,
    },
    LangSpec {
        name: "python",
        extensions: &["py", "pyi"],
        language: || tree_sitter_python::LANGUAGE.into(),
        root: "module",
        kinds: PYTHON_KINDS,
        unwrap: python_unwrap,
        significant_indent: true,
    },
    LangSpec {
        name: "go",
        extensions: &["go"],
        language: || tree_sitter_go::LANGUAGE.into(),
        root: "source_file",
        kinds: GO_KINDS,
        unwrap: NO_UNWRAP,
        significant_indent: false,
    },
    LangSpec {
        name: "json",
        extensions: &["json"],
        language: || tree_sitter_json::LANGUAGE.into(),
        root: "document",
        kinds: JSON_KINDS,
        unwrap: NO_UNWRAP,
        significant_indent: false,
    },
    LangSpec {
        name: "toml",
        extensions: &["toml"],
        language: || tree_sitter_toml_ng::LANGUAGE.into(),
        root: "document",
        kinds: TOML_KINDS,
        unwrap: NO_UNWRAP,
        significant_indent: false,
    },
    LangSpec {
        name: "yaml",
        extensions: &["yaml", "yml"],
        language: || tree_sitter_yaml::LANGUAGE.into(),
        root: "stream",
        kinds: YAML_KINDS,
        unwrap: NO_UNWRAP,
        significant_indent: true,
    },
];

pub(crate) fn spec(name: &str) -> Option<&'static LangSpec> {
    SPECS.iter().find(|s| s.name == name)
}

pub(crate) fn spec_for_extension(ext: &str) -> Option<&'static LangSpec> {
    SPECS.iter().find(|s| s.extensions.contains(&ext))
}

pub(crate) fn kind_spec(spec: &LangSpec, kind: &str) -> Option<&'static KindSpec> {
    spec.kinds.iter().find(|k| k.kind == kind)
}

/// YAML nests everything under `document` / `block_node` layers; walk through
/// them to the mapping pairs that carry identity. A comment sitting between
/// the pairs is an item of its own — dropped, a comment-only edit would have
/// no structural presence at all. A comment *inside* a pair's span never
/// reaches here: the pair is pushed whole and the mapping under it is not
/// descended, so the pair's text stays its only account.
pub(crate) fn yaml_document_pairs<'tree>(
    node: tree_sitter::Node<'tree>,
) -> Vec<tree_sitter::Node<'tree>> {
    fn descend<'tree>(node: tree_sitter::Node<'tree>, out: &mut Vec<tree_sitter::Node<'tree>>) {
        match node.kind() {
            "document" | "block_node" => {
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    descend(child, out);
                }
            }
            "block_mapping" => {
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    if matches!(child.kind(), "block_mapping_pair" | "comment") {
                        out.push(child);
                    }
                }
            }
            _ => out.push(node),
        }
    }
    let mut out = Vec::new();
    descend(node, &mut out);
    out
}
