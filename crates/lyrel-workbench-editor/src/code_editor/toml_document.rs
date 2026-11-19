//! TOML 文档：一份共享逐行保真源码，加上按 revision 派生的无损结构视图。
//!
//! 源码始终是同一 [`lyrel_markdown_editor::Document`] 的 plain-source
//! presentation；结构节点只保存源码范围与显示文本，不复制第二份可变内容，
//! 也不通过 `toml_edit::DocumentMut` 整篇回写。ordinary table、inline
//! table、array of tables、dotted/quoted key 与 date/time 等标量都保留原
//! 拼写和形态。
//!
//! 文档编排（`Document` 订阅、debounce 单飞后台解析、取消与 revision 守卫）
//! 与 presentation 壳（模式切换、展开导航、滚动同步、源码定位、完整值与
//! 结构编辑会话）都在 `structured_document`／`structured_view`；这个模块只
//! 提供 TOML 的格式壳：解析、行呈现、编辑命令与 mutation 规划。无损
//! table/AoT/inline/key 编辑由 `lyrel-document` 的 TOML 引擎规划成局部
//! splice，经共享 `Document::replace_ranges` 成为一条普通 history。

use std::ops::Range;

use gpui_kit::SharedString;
use gpui_kit::component::Theme;
use lyrel_document::state::CancelToken;
use lyrel_document::toml::mutation::{self, Mutation, MutationError, TomlTarget};
use lyrel_document::toml::projection::{
    self, ProjectionFailure, STRUCTURE_INPUT_BYTES, STRUCTURE_NODES, STRUCTURE_PATH_BYTES,
    TomlNodeKind, TomlProjection, TomlValueKind,
};
use lyrel_document::tree::Treelike;

use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};

use super::structure_edit::EditField;
use super::structured_document::{StructuredDocument, StructuredDocumentFormat};
use super::structured_view::{
    AUTO_EXPAND_CHILDREN, EditButton, EditFlow, EditPanelDesc, EditPlan, NodeIdentity, NodeRow,
    SelectionRestore, StructuredMode, StructuredMutationError, StructuredView,
    StructuredViewFormat,
};

/// 打开时自动展开根附近的浅层容器，复现上游的浅层可读树。
const AUTO_EXPAND_DEPTH: usize = 3;
const AUTO_EXPAND_NODES: usize = 256;

/// TOML 格式壳：单位结构文档/视图适配器；所有编排与呈现状态都属于共享模块。
#[derive(Clone, Copy, Debug)]
pub struct TomlShell;

impl StructuredDocumentFormat for TomlShell {
    type Projection = TomlProjection;
    type Failure = ProjectionFailure;

    fn parse(&self, text: &str, cancel: &CancelToken) -> Result<Self::Projection, Self::Failure> {
        projection::project(text, cancel)
    }
}

/// 一个工具栏树命令：目标节点在打开面板时固定，提交时按当前 revision
/// 重新解析。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TomlEditRequest {
    Value,
    Rename,
    AddKey,
    AddTable,
    AddArray,
    AddEntry,
    Remove,
}

impl StructuredViewFormat for TomlShell {
    type EditRequest = TomlEditRequest;
    type Mutation = Mutation;
    type MutationError = MutationError;
    type Target = TomlTarget;

    const KEY_CONTEXT: &str = "TomlStructure";

    fn element_prefix(&self) -> &'static str {
        "toml"
    }

    fn format_label(&self) -> SharedString {
        "TOML".into()
    }

    fn mode_labels(&self, mode: StructuredMode) -> (SharedString, SharedString) {
        match mode {
            StructuredMode::Structure => (
                t(L10nKey::EditorTomlSource).into(),
                t(L10nKey::EditorTomlShowSource).into(),
            ),
            StructuredMode::Source => (
                t(L10nKey::EditorTomlStructure).into(),
                t(L10nKey::EditorTomlShowStructure).into(),
            ),
        }
    }

    fn node_count(&self, projection: &Self::Projection) -> usize {
        projection.node_count()
    }

    fn display_node_count(&self, projection: &Self::Projection) -> Option<usize> {
        Some(projection.node_count())
    }

    fn count_notice(&self, projection: &Self::Projection) -> Option<SharedString> {
        self.display_node_count(projection)
            .map(|count| t_fmt(L10nKey::EditorTomlNodes, &[("count", &count.to_string())]).into())
    }

    /// 预算失败或整份 TOML 无法解析时的可见说明。
    fn fatal_error(
        &self,
        failure: Option<&Self::Failure>,
        projection: Option<&Self::Projection>,
    ) -> Option<(SharedString, SharedString)> {
        if let Some(failure) = failure {
            let message = match failure {
                ProjectionFailure::Cancelled => return None,
                ProjectionFailure::InputTooLarge => t_fmt(
                    L10nKey::EditorTomlInputLimit,
                    &[("mib", &(STRUCTURE_INPUT_BYTES / (1024 * 1024)).to_string())],
                )
                .into(),
                ProjectionFailure::TooManyNodes => t_fmt(
                    L10nKey::EditorTomlNodeLimit,
                    &[("count", &STRUCTURE_NODES.to_string())],
                )
                .into(),
                ProjectionFailure::PathsTooLarge => t_fmt(
                    L10nKey::EditorTomlPathLimit,
                    &[("mib", &(STRUCTURE_PATH_BYTES / (1024 * 1024)).to_string())],
                )
                .into(),
            };
            return Some((t(L10nKey::EditorTomlStructureUnavailable).into(), message));
        }
        let projection = projection?;
        let error = projection.error()?;
        Some((
            t(L10nKey::EditorTomlInvalid).into(),
            t_fmt(
                L10nKey::EditorTomlInvalidDetail,
                &[
                    ("line", &error.line.to_string()),
                    ("column", &error.column.to_string()),
                    ("message", &error.message),
                ],
            )
            .into(),
        ))
    }

    fn node_identity<'a>(
        &self,
        projection: &'a Self::Projection,
        index: usize,
    ) -> Option<NodeIdentity<'a>> {
        let node = projection.nodes().get(index)?;
        Some(NodeIdentity {
            path: &node.path,
            range: node.range.clone(),
            detail: node.detail.clone(),
            has_detail: node.has_source_detail(),
        })
    }

    fn node_row(
        &self,
        projection: &Self::Projection,
        index: usize,
        theme: &Theme,
    ) -> Option<NodeRow> {
        let node = projection.nodes().get(index)?;
        let color = match node.kind {
            TomlNodeKind::Value(TomlValueKind::String) => theme.green,
            TomlNodeKind::Value(TomlValueKind::Integer)
            | TomlNodeKind::Value(TomlValueKind::Float) => theme.blue,
            TomlNodeKind::Value(TomlValueKind::Boolean) => theme.magenta,
            TomlNodeKind::Value(TomlValueKind::Datetime) => theme.cyan,
            TomlNodeKind::Value(TomlValueKind::Array)
            | TomlNodeKind::Value(TomlValueKind::InlineTable) => theme.muted_foreground,
            TomlNodeKind::Table
            | TomlNodeKind::ArrayOfTables
            | TomlNodeKind::ArrayOfTablesEntry => theme.yellow,
            TomlNodeKind::Root => theme.foreground,
        };
        Some(NodeRow {
            key: node.key.clone().into(),
            value: node.value.clone().into(),
            color,
            badge: None,
            record_line: None,
            error: None,
            aria_label: node.path.clone().into(),
        })
    }

    /// 打开时自动展开根附近 3 层、最多 256 个节点的浅层容器。
    fn default_expansion(&self, projection: &Self::Projection, expanded: &mut [bool]) {
        let Some(root) = projection.root() else {
            return;
        };
        if root >= expanded.len() {
            return;
        }
        expanded[root] = true;
        let mut budget = AUTO_EXPAND_NODES;
        expand_children(projection, root, 1, expanded, &mut budget);
    }

    fn selection_restore(&self) -> SelectionRestore {
        SelectionRestore::Exact
    }

    fn edit_buttons(
        &self,
        projection: &Self::Projection,
        selected: Option<usize>,
    ) -> Vec<EditButton<Self::EditRequest>> {
        let mut caps_value = false;
        let mut caps_rename = false;
        let mut caps_add_key = false;
        let mut caps_add_table = false;
        let mut caps_add_array = false;
        let mut caps_add_entry = false;
        let mut caps_remove = false;
        if let Some(node) = selected.and_then(|index| projection.nodes().get(index)) {
            caps_value = node.kind.is_value() && !node.range.is_empty();
            caps_rename = match node.kind {
                TomlNodeKind::Value(_) => node.key_range.is_some(),
                TomlNodeKind::Table => node.explicit_table && !node.table_is_dotted,
                TomlNodeKind::ArrayOfTables => true,
                _ => false,
            };
            caps_add_key = match node.kind {
                TomlNodeKind::Root | TomlNodeKind::ArrayOfTablesEntry => true,
                TomlNodeKind::Table => node.explicit_table && !node.inline_context,
                TomlNodeKind::Value(TomlValueKind::InlineTable) => true,
                _ => false,
            };
            let table_container = matches!(
                node.kind,
                TomlNodeKind::Root | TomlNodeKind::Table | TomlNodeKind::ArrayOfTablesEntry
            ) && !node.inline_context
                && (node.kind != TomlNodeKind::Table || node.explicit_table);
            caps_add_table = table_container;
            caps_add_array = table_container;
            caps_add_entry = node.kind == TomlNodeKind::ArrayOfTables;
            caps_remove = match node.kind {
                TomlNodeKind::Root => false,
                TomlNodeKind::Value(_) => !node.inline_context || node.inline_direct,
                TomlNodeKind::Table => !node.inline_context,
                TomlNodeKind::ArrayOfTables | TomlNodeKind::ArrayOfTablesEntry => true,
            };
        }
        vec![
            EditButton {
                id: "edit-value",
                label: t(L10nKey::EditorTomlEditValue).into(),
                enabled: caps_value,
                flow: EditFlow::Panel(TomlEditRequest::Value),
            },
            EditButton {
                id: "rename",
                label: t(L10nKey::EditorTomlRename).into(),
                enabled: caps_rename,
                flow: EditFlow::Panel(TomlEditRequest::Rename),
            },
            EditButton {
                id: "add-key",
                label: t(L10nKey::EditorTomlAddKey).into(),
                enabled: caps_add_key,
                flow: EditFlow::Panel(TomlEditRequest::AddKey),
            },
            EditButton {
                id: "add-table",
                label: t(L10nKey::EditorTomlAddTable).into(),
                enabled: caps_add_table,
                flow: EditFlow::Panel(TomlEditRequest::AddTable),
            },
            EditButton {
                id: "add-array",
                label: t(L10nKey::EditorTomlAddArray).into(),
                enabled: caps_add_array,
                flow: EditFlow::Panel(TomlEditRequest::AddArray),
            },
            EditButton {
                id: "add-entry",
                label: t(L10nKey::EditorTomlAddEntry).into(),
                enabled: caps_add_entry,
                flow: EditFlow::Immediate(TomlEditRequest::AddEntry),
            },
            EditButton {
                id: "remove",
                label: t(L10nKey::EditorTomlRemove).into(),
                enabled: caps_remove,
                flow: EditFlow::Immediate(TomlEditRequest::Remove),
            },
        ]
    }

    fn describe_edit(
        &self,
        request: &Self::EditRequest,
        projection: &Self::Projection,
        text: &str,
        selected: Option<usize>,
    ) -> Option<EditPlan<Self::Target>> {
        match request {
            TomlEditRequest::Value => {
                let node = selected.and_then(|index| projection.nodes().get(index))?;
                if !node.kind.is_value() || node.range.is_empty() {
                    return None;
                }
                let raw = text.get(node.range.clone())?;
                Some(EditPlan::Panel(EditPanelDesc {
                    title: t_fmt(L10nKey::EditorTomlEditValueTitle, &[("path", &node.path)]).into(),
                    key: None,
                    value: EditField::new(
                        t(L10nKey::EditorTomlValueName),
                        t(L10nKey::EditorTomlValuePlaceholder),
                        raw,
                    ),
                    target: TomlTarget::of(node),
                    notice: None,
                    multiline: false,
                }))
            }
            TomlEditRequest::Rename => {
                let node = selected.and_then(|index| projection.nodes().get(index))?;
                let key_text = node.key_text.clone()?;
                let (label, placeholder) = match node.kind {
                    TomlNodeKind::Table => (
                        L10nKey::EditorTomlTableName,
                        L10nKey::EditorTomlTablePlaceholder,
                    ),
                    TomlNodeKind::ArrayOfTables => (
                        L10nKey::EditorTomlArrayName,
                        L10nKey::EditorTomlArrayPlaceholder,
                    ),
                    _ => (
                        L10nKey::EditorTomlKeyName,
                        L10nKey::EditorTomlKeyPlaceholder,
                    ),
                };
                Some(EditPlan::Panel(EditPanelDesc {
                    title: t_fmt(L10nKey::EditorTomlRenameTitle, &[("path", &node.path)]).into(),
                    key: None,
                    value: EditField::new(t(label), t(placeholder), key_text),
                    target: TomlTarget::of(node),
                    notice: None,
                    multiline: false,
                }))
            }
            TomlEditRequest::AddKey => {
                let node = selected.and_then(|index| projection.nodes().get(index))?;
                Some(EditPlan::Panel(EditPanelDesc {
                    title: t_fmt(L10nKey::EditorTomlAddKeyTitle, &[("path", &node.path)]).into(),
                    key: Some(EditField::new(
                        t(L10nKey::EditorTomlKeyName),
                        t(L10nKey::EditorTomlKeyPlaceholder),
                        "",
                    )),
                    value: EditField::new(
                        t(L10nKey::EditorTomlValueName),
                        t(L10nKey::EditorTomlValuePlaceholder),
                        "",
                    ),
                    target: TomlTarget::of(node),
                    notice: None,
                    multiline: false,
                }))
            }
            TomlEditRequest::AddTable => {
                let node = selected.and_then(|index| projection.nodes().get(index))?;
                Some(EditPlan::Panel(EditPanelDesc {
                    title: t_fmt(L10nKey::EditorTomlAddTableTitle, &[("path", &node.path)]).into(),
                    key: None,
                    value: EditField::new(
                        t(L10nKey::EditorTomlTableName),
                        t(L10nKey::EditorTomlTablePlaceholder),
                        "",
                    ),
                    target: TomlTarget::of(node),
                    notice: None,
                    multiline: false,
                }))
            }
            TomlEditRequest::AddArray => {
                let node = selected.and_then(|index| projection.nodes().get(index))?;
                Some(EditPlan::Panel(EditPanelDesc {
                    title: t_fmt(L10nKey::EditorTomlAddArrayTitle, &[("path", &node.path)]).into(),
                    key: None,
                    value: EditField::new(
                        t(L10nKey::EditorTomlArrayName),
                        t(L10nKey::EditorTomlArrayPlaceholder),
                        "",
                    ),
                    target: TomlTarget::of(node),
                    notice: None,
                    multiline: false,
                }))
            }
            TomlEditRequest::AddEntry => {
                let node = selected.and_then(|index| projection.nodes().get(index))?;
                Some(EditPlan::Immediate {
                    target: TomlTarget::of(node),
                })
            }
            TomlEditRequest::Remove => {
                let node = selected.and_then(|index| projection.nodes().get(index))?;
                Some(EditPlan::Immediate {
                    target: TomlTarget::of(node),
                })
            }
        }
    }

    fn build_mutation(
        &self,
        request: &Self::EditRequest,
        target: &Self::Target,
        key: Option<String>,
        value: String,
    ) -> Result<Self::Mutation, Self::MutationError> {
        Ok(match request {
            TomlEditRequest::Value => Mutation::SetValue {
                target: target.clone(),
                value,
            },
            TomlEditRequest::Rename => Mutation::Rename {
                target: target.clone(),
                key: value,
            },
            TomlEditRequest::AddKey => Mutation::AddKey {
                container: target.clone(),
                key: key.unwrap_or_default(),
                value,
            },
            TomlEditRequest::AddTable => Mutation::AddTable {
                container: target.clone(),
                key: value,
            },
            TomlEditRequest::AddArray => Mutation::AddArray {
                container: target.clone(),
                key: value,
            },
            TomlEditRequest::AddEntry => Mutation::AddEntry {
                array: target.clone(),
            },
            TomlEditRequest::Remove => Mutation::Remove {
                target: target.clone(),
            },
        })
    }

    fn plan_mutation(
        &self,
        text: &str,
        projection: &Self::Projection,
        mutation: &Self::Mutation,
    ) -> Result<Vec<(Range<usize>, String)>, Self::MutationError> {
        let _ = projection;
        mutation::plan(text, mutation)
    }

    fn mutation_message(
        &self,
        error: StructuredMutationError<Self::MutationError>,
    ) -> SharedString {
        match error {
            StructuredMutationError::Stale => t(L10nKey::EditorTomlEditStale).into(),
            StructuredMutationError::Plan(MutationError::Stale) => {
                t(L10nKey::EditorTomlEditStale).into()
            }
            StructuredMutationError::Plan(MutationError::Unsupported) => {
                t(L10nKey::EditorTomlEditUnsupported).into()
            }
            StructuredMutationError::Plan(MutationError::InvalidKey(message)) => {
                t_fmt(L10nKey::EditorTomlEditInvalidKey, &[("message", &message)]).into()
            }
            StructuredMutationError::Plan(MutationError::InvalidValue(message)) => t_fmt(
                L10nKey::EditorTomlEditInvalidValue,
                &[("message", &message)],
            )
            .into(),
            StructuredMutationError::Plan(MutationError::DuplicateKey(key)) => {
                t_fmt(L10nKey::EditorTomlEditDuplicate, &[("key", &key)]).into()
            }
            StructuredMutationError::Plan(MutationError::ResultInvalid {
                line,
                column,
                message,
            }) => t_fmt(
                L10nKey::EditorTomlEditResultInvalid,
                &[
                    ("line", &line.to_string()),
                    ("column", &column.to_string()),
                    ("message", &message),
                ],
            )
            .into(),
            StructuredMutationError::Plan(MutationError::ResultShape) => {
                t(L10nKey::EditorTomlEditResultShape).into()
            }
        }
    }

    fn loading_label(&self) -> SharedString {
        t(L10nKey::EditorTomlParsing).into()
    }

    fn unavailable(&self) -> (SharedString, SharedString) {
        (
            t(L10nKey::EditorTomlStructureUnavailable).into(),
            t(L10nKey::EditorTomlSourceStaysEditable).into(),
        )
    }

    fn full_value_label(&self) -> SharedString {
        t(L10nKey::EditorTomlFullValue).into()
    }

    fn copy_value_label(&self) -> SharedString {
        t(L10nKey::EditorTomlCopyValue).into()
    }

    fn reveal_label(&self) -> SharedString {
        t(L10nKey::EditorTomlReveal).into()
    }
}

fn expand_children(
    projection: &TomlProjection,
    node: usize,
    depth: usize,
    expanded: &mut [bool],
    budget: &mut usize,
) {
    if depth > AUTO_EXPAND_DEPTH || *budget == 0 {
        return;
    }
    for &child in projection.children(node) {
        if projection.is_expandable(child)
            && projection.children(child).len() <= AUTO_EXPAND_CHILDREN
            && *budget > 0
        {
            expanded[child] = true;
            *budget -= 1;
            expand_children(projection, child, depth + 1, expanded, budget);
        }
    }
}

/// 一份 TOML 文件的内容：共享 Document 与按最新 revision 安装的投影。
pub type TomlDocument = StructuredDocument<TomlShell>;

/// 一个 presentation 里的 TOML 呈现：模式、源码选择、结构展开与滚动。
pub type TomlView = StructuredView<TomlShell>;

/// TOML 呈现模式：结构树或源码。
pub type TomlMode = StructuredMode;

/// 结构预览持有键盘时成立的上下文；源码面不声明它，文字编辑键不会被截走。
pub const KEY_CONTEXT: &str = <TomlShell as StructuredViewFormat>::KEY_CONTEXT;

/// 右侧菜单的 TOML 项：默认只给结构与源码切换；编辑命令由结构工具栏提供。
pub fn menu_entries(
    menu: gpui_kit::component::menu::PopupMenu,
) -> gpui_kit::component::menu::PopupMenu {
    super::structured_view::menu_entries(menu, L10nKey::EditorTomlToggleStructure)
}

/// 解析的确定性单元测试门闩，不编译进产品或独立 native 消费者。
#[cfg(any(test, feature = "test-support"))]
pub use super::structured_document::test_support;
