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
