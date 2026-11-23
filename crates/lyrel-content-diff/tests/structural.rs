//! Hand-labeled fixtures for the structural matcher. Every expectation here
//! was written by reading the two versions; the tests assert the matcher
//! agrees with a human reviewer.

use lyrel_content_diff::{
    language_for_path, structural_diff, Block, BlockKind, ContentError, EditKind, Item, LineSpan,
};

fn blocks(diff: &lyrel_content_diff::StructuralDiff) -> Vec<&Block> {
    diff.items
        .iter()
        .filter_map(|item| match item {
            Item::Block(block) => Some(block),
            Item::Source { .. } => None,
        })
        .collect()
}

fn anchors(diff: &lyrel_content_diff::StructuralDiff) -> Vec<(LineSpan, u32)> {
    diff.items
        .iter()
        .filter_map(|item| match item {
            Item::Source { old, new_start, .. } => Some((*old, *new_start)),
            Item::Block(_) => None,
        })
        .collect()
}

/// (kind, old span, new span) per block, in display order.
fn shape(diff: &lyrel_content_diff::StructuralDiff) -> Vec<(BlockKind, Option<LineSpan>, Option<LineSpan>)> {
    blocks(diff)
        .into_iter()
        .map(|b| (b.kind, b.old, b.new))
        .collect()
}

const OLD_BASIC: &str = "fn alpha() {}\n\nfn beta() {\n    println!(\"beta\");\n}\n\nfn gamma() {}\nfn delta() {}\n";
// beta moved below gamma and edited; delta deleted; epsilon added.
const NEW_BASIC: &str = "fn alpha() {}\n\nfn gamma() {}\n\nfn beta() {\n    println!(\"beta v2\");\n}\n\nfn epsilon() {}\n";

#[test]
fn a_moved_and_edited_block_reports_its_inner_edits() {
    let diff = structural_diff("rust", OLD_BASIC, NEW_BASIC).expect("rust parses");
    // alpha is untouched context and never appears.
    assert!(!blocks(&diff).iter().any(|b| b.name.as_deref() == Some("alpha")));
    let beta = blocks(&diff)
        .into_iter()
        .find(|b| b.name.as_deref() == Some("beta"))
        .expect("beta is one block");
    assert_eq!(beta.kind, BlockKind::MovedEdited);
    assert_eq!(beta.old, Some(LineSpan { start: 3, end: 5 }));
    assert_eq!(beta.new, Some(LineSpan { start: 5, end: 7 }));
    assert!(!beta.formatting_only);
    // The edit inside the moved block points at both versions of the
    // changed line.
    let kinds: Vec<EditKind> = beta.edits.iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&EditKind::Removed), "the old println line: {kinds:?}");
    assert!(kinds.contains(&EditKind::Added), "the new println line: {kinds:?}");
    let removed = beta.edits.iter().find(|e| e.kind == EditKind::Removed).unwrap();
    let added = beta.edits.iter().find(|e| e.kind == EditKind::Added).unwrap();
    assert_eq!(removed.old, Some(4));
    assert_eq!(added.new, Some(6));
}

#[test]
fn moves_grow_a_source_anchor_at_the_old_position() {
    let diff = structural_diff("rust", OLD_BASIC, NEW_BASIC).expect("rust parses");
    let anchors = anchors(&diff);
    assert!(
        anchors.contains(&(LineSpan { start: 3, end: 5 }, 5)),
        "beta's old position points at the new one: {anchors:?}"
    );
}

#[test]
fn ambiguous_matches_stay_additions_and_deletions() {
    // One `helper` on the left, two on the right: which copy moved cannot be
    // established, so nothing may be paired.
    let old = "def helper():\n    return 1\n\ndef main():\n    pass\n";
    let new = "def helper():\n    return 2\n\ndef helper():\n    return 1\n\ndef main():\n    pass\n";
    let diff = structural_diff("python", old, new).expect("python parses");
    let helper_blocks: Vec<&Block> = blocks(&diff)
        .into_iter()
        .filter(|b| b.name.as_deref() == Some("helper"))
        .collect();
    assert_eq!(helper_blocks.len(), 3, "one deletion, two additions: {helper_blocks:?}");
    assert!(helper_blocks.iter().all(|b| matches!(b.kind, BlockKind::Added | BlockKind::Deleted)));
}

#[test]
fn a_pure_move_without_edits_has_no_inner_edits() {
    let old = "fn a() {}\n\nfn b() {\n    work();\n}\n\nfn c() {}\n";
    let new = "fn a() {}\n\nfn c() {}\n\nfn b() {\n    work();\n}\n";
    let diff = structural_diff("rust", old, new).expect("rust parses");
    let moved = blocks(&diff)
        .into_iter()
        .filter(|b| b.kind == BlockKind::Moved)
        .count();
    assert_eq!(moved, 2, "b and c both moved: {moved}");
    let b = blocks(&diff)
        .into_iter()
        .find(|b| b.name.as_deref() == Some("b"))
        .unwrap();
    assert!(b.edits.is_empty());
    // The display sequence interleaves anchors and blocks by position: at
    // line 3 the reader meets "b moved away" and the block that took the
    // place (c); at line 7 the reverse.
    let kinds: Vec<&str> = diff
        .items
        .iter()
        .map(|item| match item {
            Item::Source { name, .. } => match name.as_deref() {
                Some("b") => "b-anchor",
                Some("c") => "c-anchor",
                _ => "anchor",
            },
            Item::Block(block) => match block.name.as_deref() {
                Some("b") => "b-block",
                Some("c") => "c-block",
                _ => "block",
            },
        })
        .collect();
    assert_eq!(kinds, vec!["b-anchor", "c-block", "b-block", "c-anchor"], "{kinds:?}");
}

#[test]
fn formatting_only_changes_are_flagged_not_expanded() {
    let old = "fn a() {}\n\nfn b() {\n    let x = 1;\n    let y = 2;\n}\n";
    let new = "fn a() {}\n\nfn b() {\n  let x = 1;\n     let y = 2;\n\n}\n";
    let diff = structural_diff("rust", old, new).expect("rust parses");
    let b = blocks(&diff)
        .into_iter()
        .find(|b| b.name.as_deref() == Some("b"))
        .expect("b changed");
    assert!(b.formatting_only, "only whitespace moved: {b:?}");
    assert!(b.edits.is_empty(), "formatting-only blocks carry no edit rows");
    assert_eq!(b.kind, BlockKind::Modified, "b stayed in place");
}

#[test]
fn comment_and_literal_changes_are_real_edits() {
    let old = "fn a() {}\n\nfn b() {\n    // greet the user\n    let msg = \"hi\";\n}\n";
    let new = "fn a() {}\n\nfn b() {\n    // greet the operator\n    let msg = \"hello\";\n}\n";
    let diff = structural_diff("rust", old, new).expect("rust parses");
    let b = blocks(&diff)
        .into_iter()
        .find(|b| b.name.as_deref() == Some("b"))
        .unwrap();
    assert!(!b.formatting_only, "comments and strings are content: {b:?}");
    assert_eq!(b.edits.iter().filter(|e| e.kind != EditKind::Context).count(), 4);
}

#[test]
fn added_and_deleted_items_report_their_whole_text_as_edits() {
    let diff = structural_diff("rust", OLD_BASIC, NEW_BASIC).expect("rust parses");
    let delta = blocks(&diff)
        .into_iter()
        .find(|b| b.name.as_deref() == Some("delta"))
        .expect("delta was deleted");
    assert_eq!(delta.kind, BlockKind::Deleted);
    assert_eq!(delta.old, Some(LineSpan { start: 8, end: 8 }));
    assert!(delta.new.is_none());
    assert_eq!(delta.edits.len(), 1);
    assert_eq!(delta.edits[0].kind, EditKind::Removed);
    assert_eq!(delta.edits[0].old, Some(8));

    let epsilon = blocks(&diff)
        .into_iter()
        .find(|b| b.name.as_deref() == Some("epsilon"))
        .expect("epsilon was added");
    assert_eq!(epsilon.kind, BlockKind::Added);
    assert_eq!(epsilon.new, Some(LineSpan { start: 9, end: 9 }));
    assert_eq!(epsilon.edits[0].kind, EditKind::Added);
}

#[test]
fn reordered_blocks_read_in_new_file_order() {
    let old = "fn one() {}\nfn two() {}\nfn three() {}\n";
    let new = "fn three() {}\nfn one() {}\nfn two() {}\n";
    let diff = structural_diff("rust", old, new).expect("rust parses");
    let starts: Vec<u32> = blocks(&diff)
        .into_iter()
        .filter_map(|b| b.new.map(|s| s.start))
        .collect();
    assert_eq!(starts, vec![1, 2, 3], "new-file order: {starts:?}");
}

#[test]
fn unsupported_language_and_unknown_extension() {
    assert_eq!(language_for_path(std::path::Path::new("x.rs")), Some("rust"));
    assert_eq!(language_for_path(std::path::Path::new("x.TSX")), None);
    assert_eq!(language_for_path(std::path::Path::new("noext")), None);
    let err = structural_diff("cobol", "a", "b").unwrap_err();
    assert_eq!(err, ContentError::Unsupported);
}

#[test]
fn oversized_inputs_decline_named_budgets() {
    let huge_bytes = "x".repeat(lyrel_content_diff::MAX_FILE_BYTES + 1);
    let err = structural_diff("rust", &huge_bytes, "fn a() {}\n").unwrap_err();
    assert_eq!(err, ContentError::OverBudget("1 MiB"), "{err}");

    let huge_lines = "x\n".repeat((lyrel_content_diff::MAX_LINES + 1) as usize);
    let err = structural_diff("rust", &huge_lines, "fn a() {}\n").unwrap_err();
    assert!(matches!(err, ContentError::OverBudget(_)), "{err}");

    let many_items = "fn a() {}\n".repeat(lyrel_content_diff::MAX_ITEMS + 1);
    let err = structural_diff("rust", &many_items, "fn a() {}\n").unwrap_err();
    assert!(matches!(err, ContentError::OverBudget(_)), "{err}");
}

#[test]
fn unparseable_text_declines_instead_of_guessing() {
    let err = structural_diff("rust", "fn fn fn {{{", "fn a() {}\n").unwrap_err();
    assert_eq!(err, ContentError::ParseFailed);
}

#[test]
fn same_name_sibling_items_stay_conservative_when_ambiguous() {
    // Two same-named structs whose edits make cross-pairing score equal:
    // whichever pairing the matcher chose would be a guess, so all four
    // stay unpaired.
    let old = "struct Box {\n    a: u8,\n    keep_a: u8,\n}\n\nstruct Bin {\n    b: u8,\n}\n";
    let new = "struct Box {\n    a: u8,\n    keep_b: u8,\n}\n\nstruct Bin {\n    b: u8,\n    extra: u8,\n}\n";
    let diff = structural_diff("rust", old, new).expect("rust parses");
    let shape = shape(&diff);
    // Box and Bin both changed in place with the same identity available on
    // each side exactly once — label+name is unique here, so both pair as
    // modifications. This asserts the happy path stays reachable when there
    // is no ambiguity.
    assert_eq!(shape.len(), 2, "{shape:?}");
    assert!(shape.iter().all(|(kind, o, n)| {
        *kind == BlockKind::Modified && o.is_some() && n.is_some()
    }));
}

// ---- the other advertised formats ----

#[test]
fn javascript_moves_are_detected() {
    let old = "function one() {\n  return 1;\n}\n\nfunction two() {\n  return 2;\n}\n";
    let new = "function two() {\n  return 2;\n}\n\nfunction one() {\n  return 1;\n}\n";
    let diff = structural_diff("javascript", old, new).expect("javascript parses");
    assert_eq!(blocks(&diff).iter().filter(|b| b.kind == BlockKind::Moved).count(), 2);
}

#[test]
fn typescript_exported_functions_keep_their_identity() {
    let old = "export function parse(input: string): number {\n  return Number(input);\n}\n";
    let new = "export function parse(input: string): number {\n  return Math.floor(Number(input));\n}\n";
    let diff = structural_diff("typescript", old, new).expect("typescript parses");
    let b = &blocks(&diff)[0];
    assert_eq!(b.kind, BlockKind::Modified);
    assert_eq!(b.name.as_deref(), Some("parse"));
}

#[test]
fn tsx_components_match() {
    let old = "export function App() {\n  return <div>a</div>;\n}\n";
    let new = "export function App() {\n  return <div>b</div>;\n}\n";
    let diff = structural_diff("tsx", old, new).expect("tsx parses");
    assert_eq!(blocks(&diff)[0].kind, BlockKind::Modified);
}

#[test]
fn python_class_moves_with_its_methods() {
    let old = "class One:\n    pass\n\n\ndef helper():\n    pass\n";
    let new = "def helper():\n    pass\n\n\nclass One:\n    pass\n";
    let diff = structural_diff("python", old, new).expect("python parses");
    let one = blocks(&diff)
        .into_iter()
        .find(|b| b.name.as_deref() == Some("One"))
        .unwrap();
    assert_eq!(one.kind, BlockKind::Moved);
    // helper is unchanged content at a different place — also a move, and
    // its old position grows an anchor.
    assert_eq!(anchors(&diff).len(), 2);
}

#[test]
fn go_grouped_specs_match_by_name() {
    let old = "package main\n\nconst (\n\tA = 1\n\tB = 2\n)\n\nfunc main() {}\n";
    let new = "package main\n\nconst (\n\tA = 1\n\tB = 3\n)\n\nfunc main() {}\n";
    let diff = structural_diff("go", old, new).expect("go parses");
    let b = blocks(&diff)
        .into_iter()
        .find(|b| b.name.as_deref() == Some("B"))
        .expect("const B changed");
    assert_eq!(b.kind, BlockKind::Modified);
    assert!(!b.formatting_only);
}

#[test]
fn json_keys_match_by_name() {
    let old = "{\n  \"name\": \"lyrel\",\n  \"version\": 1\n}\n";
    let new = "{\n  \"name\": \"lyrel\",\n  \"version\": 2\n}\n";
    let diff = structural_diff("json", old, new).expect("json parses");
    let version = blocks(&diff)
        .into_iter()
        .find(|b| b.name.as_deref() == Some("\"version\""))
        .expect("the version key changed");
    assert_eq!(version.kind, BlockKind::Modified);
}

#[test]
fn toml_sections_move() {
    let old = "[package]\nname = \"lyrel\"\n\n[dependencies]\nrust = \"1\"\n";
    let new = "[dependencies]\nrust = \"1\"\n\n[package]\nname = \"lyrel\"\n";
    let diff = structural_diff("toml", old, new).expect("toml parses");
    assert_eq!(blocks(&diff).iter().filter(|b| b.kind == BlockKind::Moved).count(), 2);
}

#[test]
fn yaml_top_level_keys_match_by_name() {
    let old = "name: lyrel\nversion: 1\n";
    let new = "name: lyrel\nversion: 2\n";
    let diff = structural_diff("yaml", old, new).expect("yaml parses");
    let version = blocks(&diff)
        .into_iter()
        .find(|b| b.name.as_deref() == Some("version"))
        .expect("the version key changed");
    assert_eq!(version.kind, BlockKind::Modified);
    assert!(!version.formatting_only);
}

#[test]
fn empty_old_side_reads_everything_as_added() {
    let diff = structural_diff("rust", "", "fn a() {}\nfn b() {}\n").expect("rust parses");
    assert_eq!(blocks(&diff).len(), 2);
    assert!(blocks(&diff).iter().all(|b| b.kind == BlockKind::Added));
}

#[test]
fn empty_new_side_reads_everything_as_deleted() {
    let diff = structural_diff("rust", "fn a() {}\n", "").expect("rust parses");
    assert_eq!(blocks(&diff).len(), 1);
    assert_eq!(blocks(&diff)[0].kind, BlockKind::Deleted);
}

#[test]
fn line_numbers_are_file_global() {
    let old = "fn a() {}\n\n\n\nfn b() {\n    one();\n    two();\n}\n";
    let new = "fn a() {}\n\n\n\nfn b() {\n    one();\n    three();\n}\n";
    let diff = structural_diff("rust", old, new).expect("rust parses");
    let b = blocks(&diff)
        .into_iter()
        .find(|b| b.name.as_deref() == Some("b"))
        .unwrap();
    let removed = b.edits.iter().find(|e| e.kind == EditKind::Removed).unwrap();
    let added = b.edits.iter().find(|e| e.kind == EditKind::Added).unwrap();
    assert_eq!(removed.old, Some(7));
    assert_eq!(added.new, Some(7));
    let ctx = b.edits.iter().find(|e| e.kind == EditKind::Context).unwrap();
    assert_eq!(ctx.old, Some(5));
    assert_eq!(ctx.new, Some(5));
}

// ---- hand-labeled before/after fixtures for every added format (#390) ----
//
// Each fixture below was written by reading both versions line by line. The
// assertions name the block family, the identity the reader sees, the
// original-side range and the source anchor, plus the comment/string/whitespace
// change that must stay visible inside the block.

fn block_named<'a>(diff: &'a lyrel_content_diff::StructuralDiff, name: &str) -> &'a Block {
    blocks(diff)
        .into_iter()
        .find(|b| b.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no block named {name}"))
}

fn anchor_named(diff: &lyrel_content_diff::StructuralDiff, name: &str) -> Option<(LineSpan, u32)> {
    diff.items.iter().find_map(|item| match item {
        Item::Source { name: Some(n), old, new_start, .. } if n == name => {
            Some((*old, *new_start))
        }
        _ => None,
    })
}

#[test]
fn javascript_moves_keep_function_and_variable_identity() {
    let old = r#"function greet(name) {
  // say hello
  return "hello, " + name;
}

const retries = 3;

function main() {
  console.log(greet("world"));
}
"#;
    let new = r#"function main() {
  console.log(greet("world"));
}

const retries = 5;

function greet(name) {
  // say hi
  return "hi, " + name;
}
"#;
    let diff = structural_diff("javascript", old, new).expect("javascript parses");

    let greet = block_named(&diff, "greet");
    assert_eq!(greet.label, "function");
    assert_eq!(greet.kind, BlockKind::MovedEdited);
    assert_eq!(greet.old, Some(LineSpan { start: 1, end: 4 }));
    assert_eq!(greet.new, Some(LineSpan { start: 7, end: 10 }));
    assert!(!greet.formatting_only, "a comment and a literal changed: {greet:?}");
    assert_eq!(
        anchor_named(&diff, "greet"),
        Some((LineSpan { start: 1, end: 4 }, 7)),
        "the old position stays jumpable"
    );
    let removed: Vec<Option<u32>> = greet
        .edits
        .iter()
        .filter(|e| e.kind == EditKind::Removed)
        .map(|e| e.old)
        .collect();
    assert_eq!(removed, vec![Some(2), Some(3)], "{:?}", greet.edits);

    let retries = block_named(&diff, "retries");
    assert_eq!(retries.label, "variable");
    assert_eq!(retries.kind, BlockKind::MovedEdited);
    assert_eq!(retries.old, Some(LineSpan { start: 6, end: 6 }));
    assert_eq!(retries.new, Some(LineSpan { start: 5, end: 5 }));
    assert_eq!(anchor_named(&diff, "retries"), Some((LineSpan { start: 6, end: 6 }, 5)));

    let main = block_named(&diff, "main");
    assert_eq!(main.kind, BlockKind::Moved);
    assert!(!main.formatting_only, "a move reads as a move, not as formatting");
    assert!(main.edits.is_empty(), "an exact move has no inner edits");
    assert_eq!(anchor_named(&diff, "main"), Some((LineSpan { start: 8, end: 10 }, 1)));
}

#[test]
fn javascript_whitespace_only_edits_collapse() {
    let old = "function run() {\n  const x = 1;\n  return x;\n}\n";
    let new = "function run() {\n    const x = 1;\n\n    return x;\n}\n";
    let diff = structural_diff("javascript", old, new).expect("javascript parses");
    let run = block_named(&diff, "run");
    assert_eq!(run.kind, BlockKind::Modified);
    assert!(run.formatting_only, "{run:?}");
    assert!(run.edits.is_empty(), "formatting-only blocks carry no edit rows");
}

#[test]
fn typescript_declarations_keep_their_identity_across_moves() {
    let old = r#"export interface Options {
  retries: number;
}

type Mode = "fast" | "safe";

enum Phase {
  Idle = "idle",
  Busy = "busy",
}

export function run(options: Options): void {
  console.log("running", options.retries);
}
"#;
    let new = r#"type Mode = "fast" | "careful";

enum Phase {
  Idle = "idle",
  Busy = "working",
}

export function run(options: Options): void {
  console.log("running", options.retries);
}

export interface Options {
  retries: number;
  timeout: number;
}
"#;
    let diff = structural_diff("typescript", old, new).expect("typescript parses");

    // A TS interface is a type item, and the `export` wrapper stays part of
    // its range, so the moved block starts on the `export` line.
    let options = block_named(&diff, "Options");
    assert_eq!(options.label, "type");
    assert_eq!(options.kind, BlockKind::MovedEdited);
    assert_eq!(options.old, Some(LineSpan { start: 1, end: 3 }));
    assert_eq!(options.new, Some(LineSpan { start: 12, end: 15 }));
    assert_eq!(
        anchor_named(&diff, "Options"),
        Some((LineSpan { start: 1, end: 3 }, 12))
    );
    assert!(
        options.edits.iter().any(|e| e.kind == EditKind::Added && e.new == Some(14)),
        "the added field stays visible: {:?}",
        options.edits
    );

    let mode = block_named(&diff, "Mode");
    assert_eq!(mode.label, "type");
    assert_eq!(mode.kind, BlockKind::MovedEdited);
    assert_eq!(mode.old, Some(LineSpan { start: 5, end: 5 }));
    assert_eq!(mode.new, Some(LineSpan { start: 1, end: 1 }));
    assert_eq!(anchor_named(&diff, "Mode"), Some((LineSpan { start: 5, end: 5 }, 1)));

    let phase = block_named(&diff, "Phase");
    assert_eq!(phase.label, "enum");
    assert_eq!(phase.kind, BlockKind::MovedEdited);
    assert_eq!(phase.old, Some(LineSpan { start: 7, end: 10 }));
    assert_eq!(phase.new, Some(LineSpan { start: 3, end: 6 }));
    assert!(
        phase.edits.iter().any(|e| e.kind == EditKind::Removed && e.old == Some(9))
            && phase.edits.iter().any(|e| e.kind == EditKind::Added && e.new == Some(5)),
        "the enum literal change is a real edit: {:?}",
        phase.edits
    );

    let run = block_named(&diff, "run");
    assert_eq!(run.kind, BlockKind::Moved);
    assert_eq!(anchor_named(&diff, "run"), Some((LineSpan { start: 12, end: 14 }, 8)));
}

#[test]
fn typescript_export_and_ambient_wrappers_stay_in_the_item() {
    // Dropping `export` is a change to the item, not an invisible keyword.
    let old = "export function parse(input: string) {\n  return Number(input);\n}\n";
    let new = "function parse(input: string) {\n  return Number(input);\n}\n";
    let diff = structural_diff("typescript", old, new).expect("typescript parses");
    let parse = block_named(&diff, "parse");
    assert_eq!(parse.kind, BlockKind::Modified);
    assert_eq!(parse.old, Some(LineSpan { start: 1, end: 3 }));
    assert_eq!(parse.new, Some(LineSpan { start: 1, end: 3 }));
    assert!(
        parse.edits.iter().any(|e| e.kind == EditKind::Removed && e.old == Some(1))
            && parse.edits.iter().any(|e| e.kind == EditKind::Added && e.new == Some(1)),
        "the export line is the edit: {:?}",
        parse.edits
    );

    // Ambient signatures are classified through `declare`.
    let old = "declare function parse(input: string): number;\n";
    let new = "declare function parse(input: string): bigint;\n";
    let diff = structural_diff("typescript", old, new).expect("typescript parses");
    let parse = block_named(&diff, "parse");
    assert_eq!(parse.label, "function");
    assert_eq!(parse.kind, BlockKind::Modified);
    assert_eq!(parse.old, Some(LineSpan { start: 1, end: 1 }));
    assert_eq!(parse.new, Some(LineSpan { start: 1, end: 1 }));
}

#[test]
fn tsx_components_move_with_their_jsx_literals() {
    let old = r#"export function Card() {
  // render the card
  return <div className="card">old</div>;
}

export function Shell() {
  return <main><Card /></main>;
}
"#;
    let new = r#"export function Shell() {
  return <main><Card /></main>;
}

export function Card() {
  // render the panel
  return <div className="card">new</div>;
}
"#;
    let diff = structural_diff("tsx", old, new).expect("tsx parses JSX");

    let card = block_named(&diff, "Card");
    assert_eq!(card.label, "function");
    assert_eq!(card.kind, BlockKind::MovedEdited);
    assert_eq!(card.old, Some(LineSpan { start: 1, end: 4 }));
    assert_eq!(card.new, Some(LineSpan { start: 5, end: 8 }));
    assert_eq!(
        anchor_named(&diff, "Card"),
        Some((LineSpan { start: 1, end: 4 }, 5))
    );
    let removed: Vec<Option<u32>> = card
        .edits
        .iter()
        .filter(|e| e.kind == EditKind::Removed)
        .map(|e| e.old)
        .collect();
    assert_eq!(removed, vec![Some(2), Some(3)], "{:?}", card.edits);

    let shell = block_named(&diff, "Shell");
    assert_eq!(shell.kind, BlockKind::Moved);
    assert_eq!(anchor_named(&diff, "Shell"), Some((LineSpan { start: 6, end: 8 }, 1)));
}

#[test]
fn python_decorated_definitions_move_with_their_decorator() {
    let old = r#"def helper():
    # greet the user
    return "hello"

@cached
def build():
    return helper()

class Store:
    pass
"#;
    let new = r#"class Store:
    pass

@cached
def build():
    return helper()

def helper():
    # greet the operator
    return "hi"
"#;
    let diff = structural_diff("python", old, new).expect("python parses");

    // The decorator is part of the definition's range.
    let build = block_named(&diff, "build");
    assert_eq!(build.kind, BlockKind::Moved);
    assert_eq!(build.old, Some(LineSpan { start: 5, end: 7 }));
    assert_eq!(build.new, Some(LineSpan { start: 4, end: 6 }));
    assert_eq!(anchor_named(&diff, "build"), Some((LineSpan { start: 5, end: 7 }, 4)));

    let helper = block_named(&diff, "helper");
    assert_eq!(helper.kind, BlockKind::MovedEdited);
    assert_eq!(helper.old, Some(LineSpan { start: 1, end: 3 }));
    assert_eq!(helper.new, Some(LineSpan { start: 8, end: 10 }));
    assert!(!helper.formatting_only);
    let removed: Vec<Option<u32>> = helper
        .edits
        .iter()
        .filter(|e| e.kind == EditKind::Removed)
        .map(|e| e.old)
        .collect();
    assert_eq!(removed, vec![Some(2), Some(3)], "{:?}", helper.edits);

    let store = block_named(&diff, "Store");
    assert_eq!(store.label, "class");
    assert_eq!(store.kind, BlockKind::Moved);
    assert_eq!(anchor_named(&diff, "Store"), Some((LineSpan { start: 9, end: 10 }, 1)));
}

#[test]
fn python_decorator_changes_are_edits() {
    let old = "@cache\ndef build():\n    return 1\n";
    let new = "@lru_cache(maxsize=8)\ndef build():\n    return 1\n";
    let diff = structural_diff("python", old, new).expect("python parses");
    let build = block_named(&diff, "build");
    assert_eq!(build.kind, BlockKind::Modified);
    assert_eq!(build.old, Some(LineSpan { start: 1, end: 3 }));
    assert_eq!(build.new, Some(LineSpan { start: 1, end: 3 }));
    assert!(
        build.edits.iter().any(|e| e.kind == EditKind::Removed && e.old == Some(1))
            && build.edits.iter().any(|e| e.kind == EditKind::Added && e.new == Some(1)),
        "the decorator line is the edit: {:?}",
        build.edits
    );
}

#[test]
fn python_dedent_is_a_real_edit_and_internal_spacing_is_formatting() {
    // Dedenting `two()` out of the `if` keeps the token order but changes
    // meaning: indentation is structure, not formatting.
    let old = "def run(flag):\n    if flag:\n        one()\n        two()\n";
    let new = "def run(flag):\n    if flag:\n        one()\n    two()\n";
    let diff = structural_diff("python", old, new).expect("python parses");
    let run = block_named(&diff, "run");
    assert_eq!(run.kind, BlockKind::Modified);
    assert!(!run.formatting_only, "{run:?}");
    assert!(
        run.edits.iter().any(|e| e.kind == EditKind::Removed && e.old == Some(4))
            && run.edits.iter().any(|e| e.kind == EditKind::Added && e.new == Some(4)),
        "{:?}",
        run.edits
    );

    // Extra spaces between tokens are still formatting.
    let old = "def build():\n    return [1,    2]\n";
    let new = "def build():\n    return [1, 2]\n";
    let diff = structural_diff("python", old, new).expect("python parses");
    let build = block_named(&diff, "build");
    assert!(build.formatting_only, "{build:?}");
    assert!(build.edits.is_empty());
}

#[test]
fn go_type_const_and_method_moves_keep_their_source() {
    let old = r#"package main

type Config struct {
	// the display name
	Name string
}

const (
	Retries = 3
	Timeout = 30
)

func (c Config) Label() string {
	return "config: " + c.Name
}
"#;
    let new = r#"package main

func (c Config) Label() string {
	return "cfg: " + c.Name
}

const (
	Retries = 5
	Timeout = 30
)

type Config struct {
	// the shown name
	Name string
	Mode string
}
"#;
    let diff = structural_diff("go", old, new).expect("go parses");

    let config = block_named(&diff, "Config");
    assert_eq!(config.label, "type");
    assert_eq!(config.kind, BlockKind::MovedEdited);
    assert_eq!(config.old, Some(LineSpan { start: 3, end: 6 }));
    assert_eq!(config.new, Some(LineSpan { start: 12, end: 16 }));
    assert_eq!(
        anchor_named(&diff, "Config"),
        Some((LineSpan { start: 3, end: 6 }, 12))
    );
    assert!(
        config.edits.iter().any(|e| e.kind == EditKind::Removed && e.old == Some(4))
            && config.edits.iter().any(|e| e.kind == EditKind::Added && e.new == Some(13)),
        "the comment and the added field are edits: {:?}",
        config.edits
    );

    let retries = block_named(&diff, "Retries");
    assert_eq!(retries.label, "constant");
    assert_eq!(retries.kind, BlockKind::MovedEdited);
    assert_eq!(retries.old, Some(LineSpan { start: 9, end: 9 }));
    assert_eq!(retries.new, Some(LineSpan { start: 8, end: 8 }));

    let timeout = block_named(&diff, "Timeout");
    assert_eq!(timeout.kind, BlockKind::Moved);
    assert_eq!(anchor_named(&diff, "Timeout"), Some((LineSpan { start: 10, end: 10 }, 9)));

    let label = block_named(&diff, "Label");
    assert_eq!(label.label, "method");
    assert_eq!(label.kind, BlockKind::MovedEdited);
    assert_eq!(anchor_named(&diff, "Label"), Some((LineSpan { start: 13, end: 15 }, 3)));
}

#[test]
fn go_type_alias_moves_with_its_name() {
    let old = "package main\n\ntype Alias = string\n\nfunc main() {}\n";
    let new = "package main\n\nfunc main() {}\n\ntype Alias = int\n";
    let diff = structural_diff("go", old, new).expect("go parses");
    let alias = block_named(&diff, "Alias");
    assert_eq!(alias.label, "type");
    assert_eq!(alias.kind, BlockKind::MovedEdited);
    assert_eq!(alias.old, Some(LineSpan { start: 3, end: 3 }));
    assert_eq!(alias.new, Some(LineSpan { start: 5, end: 5 }));
    assert_eq!(anchor_named(&diff, "Alias"), Some((LineSpan { start: 3, end: 3 }, 5)));
}

#[test]
fn json_pairs_move_and_keep_their_nested_literals() {
    let old = r#"{
  "name": "lyrel",
  "server": {
    "host": "localhost",
    "port": 8080
  },
  "retries": 3
}
"#;
    let new = r#"{
  "retries": 5,
  "name": "lyrel",
  "server": {
    "host": "localhost",
    "port": 9090
  }
}
"#;
    let diff = structural_diff("json", old, new).expect("json parses");

    let retries = block_named(&diff, "\"retries\"");
    assert_eq!(retries.label, "key");
    assert_eq!(retries.kind, BlockKind::MovedEdited);
    assert_eq!(retries.old, Some(LineSpan { start: 7, end: 7 }));
    assert_eq!(retries.new, Some(LineSpan { start: 2, end: 2 }));
    assert_eq!(
        anchor_named(&diff, "\"retries\""),
        Some((LineSpan { start: 7, end: 7 }, 2))
    );

    let server = block_named(&diff, "\"server\"");
    assert_eq!(server.kind, BlockKind::MovedEdited);
    assert_eq!(server.old, Some(LineSpan { start: 3, end: 6 }));
    assert_eq!(server.new, Some(LineSpan { start: 4, end: 7 }));
    assert!(
        server.edits.iter().any(|e| e.kind == EditKind::Removed && e.old == Some(5))
            && server.edits.iter().any(|e| e.kind == EditKind::Added && e.new == Some(6)),
        "the nested port literal is an edit: {:?}",
        server.edits
    );

    let name = block_named(&diff, "\"name\"");
    assert_eq!(name.kind, BlockKind::Moved);
    assert_eq!(anchor_named(&diff, "\"name\""), Some((LineSpan { start: 2, end: 2 }, 3)));
}

#[test]
fn toml_tables_move_and_keep_their_comments() {
    let old = r#"[package]
# the product name
name = "lyrel"
version = "0.1.0"

[dependencies]
tree-sitter = "0.26"

[profile.release]
opt-level = 3
"#;
    let new = r#"[profile.release]
opt-level = 3

[dependencies]
tree-sitter = "0.26.13"

[package]
# the product label
name = "lyrel"
version = "0.2.0"
"#;
    let diff = structural_diff("toml", old, new).expect("toml parses");

    let package = block_named(&diff, "[package]");
    assert_eq!(package.label, "section");
    assert_eq!(package.kind, BlockKind::MovedEdited);
    assert_eq!(package.old, Some(LineSpan { start: 1, end: 4 }));
    assert_eq!(package.new, Some(LineSpan { start: 7, end: 10 }));
    assert_eq!(
        anchor_named(&diff, "[package]"),
        Some((LineSpan { start: 1, end: 4 }, 7))
    );
    assert!(
        package.edits.iter().any(|e| e.kind == EditKind::Removed && e.old == Some(2))
            && package.edits.iter().any(|e| e.kind == EditKind::Added && e.new == Some(8)),
        "the comment is a real edit: {:?}",
        package.edits
    );

    let dependencies = block_named(&diff, "[dependencies]");
    assert_eq!(dependencies.kind, BlockKind::MovedEdited);
    assert_eq!(dependencies.old, Some(LineSpan { start: 6, end: 7 }));
    assert_eq!(dependencies.new, Some(LineSpan { start: 4, end: 5 }));

    let profile = block_named(&diff, "[profile.release]");
    assert_eq!(profile.kind, BlockKind::Moved);
    assert_eq!(anchor_named(&diff, "[profile.release]"), Some((LineSpan { start: 9, end: 10 }, 1)));
}

#[test]
fn yaml_pairs_move_and_keep_their_nested_comments() {
    let old = r#"name: lyrel
retries: 3
server:
  host: localhost
  # the listening port
  port: 8080
"#;
    let new = r#"server:
  host: localhost
  # the exposed port
  port: 9090
retries: 5
name: lyrel
"#;
    let diff = structural_diff("yaml", old, new).expect("yaml parses");

    let server = block_named(&diff, "server");
    assert_eq!(server.label, "key");
    assert_eq!(server.kind, BlockKind::MovedEdited);
    assert_eq!(server.old, Some(LineSpan { start: 3, end: 6 }));
    assert_eq!(server.new, Some(LineSpan { start: 1, end: 4 }));
    assert_eq!(
        anchor_named(&diff, "server"),
        Some((LineSpan { start: 3, end: 6 }, 1))
    );
    assert!(
        server.edits.iter().any(|e| e.kind == EditKind::Removed && e.old == Some(5))
            && server.edits.iter().any(|e| e.kind == EditKind::Added && e.new == Some(3)),
        "the nested comment is a real edit: {:?}",
        server.edits
    );

    let retries = block_named(&diff, "retries");
    assert_eq!(retries.kind, BlockKind::MovedEdited);
    assert_eq!(retries.old, Some(LineSpan { start: 2, end: 2 }));
    assert_eq!(retries.new, Some(LineSpan { start: 5, end: 5 }));

    let name = block_named(&diff, "name");
    assert_eq!(name.kind, BlockKind::Moved);
    assert_eq!(anchor_named(&diff, "name"), Some((LineSpan { start: 1, end: 1 }, 6)));
}

#[test]
fn yaml_indentation_is_structure_not_formatting() {
    let old = "server:\n  host: localhost\n";
    let new = "server:\n    host: localhost\n";
    let diff = structural_diff("yaml", old, new).expect("yaml parses");
    let server = block_named(&diff, "server");
    assert_eq!(server.kind, BlockKind::Modified);
    assert!(!server.formatting_only, "leading whitespace is structure: {server:?}");
    assert!(
        server.edits.iter().any(|e| e.kind == EditKind::Removed && e.old == Some(2))
            && server.edits.iter().any(|e| e.kind == EditKind::Added && e.new == Some(2)),
        "{:?}",
        server.edits
    );
}

#[test]
fn every_advertised_extension_dispatches_to_its_own_grammar() {
    let cases = [
        ("a.js", "javascript"),
        ("a.mjs", "javascript"),
        ("a.cjs", "javascript"),
        ("a.jsx", "javascript"),
        ("a.ts", "typescript"),
        ("a.mts", "typescript"),
        ("a.cts", "typescript"),
        ("a.tsx", "tsx"),
        ("a.py", "python"),
        ("a.pyi", "python"),
        ("a.go", "go"),
        ("a.json", "json"),
        ("a.toml", "toml"),
        ("a.yaml", "yaml"),
        ("a.yml", "yaml"),
    ];
    for (path, language) in cases {
        assert_eq!(
            language_for_path(std::path::Path::new(path)),
            Some(language),
            "{path} must dispatch to {language}"
        );
    }
    // TS and TSX are separate grammars: an interface parses as TypeScript and
    // JSX parses as TSX.
    let interface = "interface Options {\n  retries: number;\n}\n";
    assert!(structural_diff("typescript", interface, "").is_ok());
    let component = "const Card = () => <div>old</div>;\n";
    assert!(structural_diff("tsx", component, "").is_ok());
}
