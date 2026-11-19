//! JSON 文档：一份共享逐行保真源码，加上按 revision 派生的无损结构视图。
//!
//! JSON／JSONC 默认显示结构树，JSONL／NDJSON 显示逐行记录；源码始终是同一
//! [`lyrel_markdown_editor::Document`] 的 plain-source presentation，保存、
//! 撤销、reload 与关闭都走既有工作台生命周期。结构节点只保存源码范围与
//! 显示文本，不复制第二份可变内容，也不通过 `serde_json::Value` 回写。
//!
//! 文档编排（`Document` 订阅、debounce 单飞后台解析、取消与 revision 守卫）
//! 与 presentation 壳（模式切换、展开导航、滚动同步、源码定位、完整值与
//! 结构编辑会话）都在 `structured_document`／`structured_view`；这个模块只
//! 提供 JSON 家族的格式壳：解析、行呈现、编辑命令与 mutation 规划。
//! JSONL／NDJSON 的逐行记录投影与记录错误行由 `lyrel-document` 的无损
//! JSON 引擎保留。

use std::ops::Range;

use gpui_kit::SharedString;
use gpui_kit::component::Theme;
use lyrel_document::json::mutation::{self, Mutation, MutationError, NodeTarget};
use lyrel_document::json::projection::{
    self, JsonFormat, JsonNodeKind, JsonProjection, ProjectionFailure, STRUCTURE_INPUT_BYTES,
    STRUCTURE_NODES, STRUCTURE_PATH_BYTES, STRUCTURE_RECORDS,
};
use lyrel_document::state::CancelToken;
use lyrel_document::tree::Treelike;

use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};

use super::structure_edit::EditField;
use super::structured_document::{StructuredDocument, StructuredDocumentFormat};
use super::structured_view::{
    AUTO_EXPAND_CHILDREN, EditButton, EditFlow, EditPanelDesc, EditPlan, NodeIdentity, NodeRow,
    SelectionRestore, StructuredMode, StructuredMutationError, StructuredView,
    StructuredViewFormat,
};

/// JSON 家族格式壳：只携带具体 `JsonFormat`（JSON／JSONC／JSONL）的文档/视图
/// 适配器；所有编排与呈现状态都属于共享模块。
#[derive(Clone, Copy, Debug)]
pub struct JsonShell {
    format: JsonFormat,
}

impl JsonShell {
    pub fn new(format: JsonFormat) -> Self {
        Self { format }
    }

    /// 这份文档的 JSON 家族格式。
    pub fn format(&self) -> JsonFormat {
        self.format
    }
}

impl StructuredDocumentFormat for JsonShell {
    type Projection = JsonProjection;
    type Failure = ProjectionFailure;

    fn parse(&self, text: &str, cancel: &CancelToken) -> Result<Self::Projection, Self::Failure> {
        projection::project(text, self.format, cancel)
    }
}

/// 一个工具栏树命令：打开面板时固定目标 occurrence 与源码 revision，
/// 提交时两者都必须仍匹配。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonEditRequest {
    Value,
    Key,
    AddMember,
    AddRecord,
    RemoveMember,
    RemoveRecord,
}

impl StructuredViewFormat for JsonShell {
    type EditRequest = JsonEditRequest;
    type Mutation = Mutation;
    type MutationError = MutationError;
    /// `AddRecord` 的目标是可选的前一条记录；`None` 表示追加到文件末尾。
    type Target = Option<NodeTarget>;

    const KEY_CONTEXT: &str = "JsonStructure";
    const STRUCTURE_UNDO_REDO: bool = true;

    fn element_prefix(&self) -> &'static str {
        "json"
    }

    fn format_label(&self) -> SharedString {
        self.format.label().into()
    }

    fn mode_labels(&self, mode: StructuredMode) -> (SharedString, SharedString) {
        match mode {
            StructuredMode::Structure => (
                t(L10nKey::EditorJsonSource).into(),
                t(L10nKey::EditorJsonShowSource).into(),
            ),
            StructuredMode::Source => (
                t(L10nKey::EditorJsonStructure).into(),
                t(L10nKey::EditorJsonShowStructure).into(),
            ),
        }
    }

    fn node_count(&self, projection: &Self::Projection) -> usize {
        projection.node_count()
    }

    fn display_node_count(&self, projection: &Self::Projection) -> Option<usize> {
        (!matches!(projection.format(), JsonFormat::JsonLines)).then_some(projection.node_count())
    }

    fn record_count(&self, projection: &Self::Projection) -> Option<usize> {
        matches!(projection.format(), JsonFormat::JsonLines).then_some(projection.record_count())
    }

    fn invalid_records(&self, projection: &Self::Projection) -> usize {
        projection.invalid_records()
    }

    fn count_notice(&self, projection: &Self::Projection) -> Option<SharedString> {
        match projection.format() {
            JsonFormat::JsonLines => self.record_count(projection).map(|count| {
                t_fmt(L10nKey::EditorJsonRecords, &[("count", &count.to_string())]).into()
            }),
            JsonFormat::Json | JsonFormat::Jsonc => {
                self.display_node_count(projection).map(|count| {
                    t_fmt(L10nKey::EditorJsonNodes, &[("count", &count.to_string())]).into()
                })
            }
        }
    }

    fn invalid_records_notice(&self, projection: &Self::Projection) -> Option<SharedString> {
        let invalid = projection.invalid_records();
        (invalid > 0).then(|| {
            t_fmt(
                L10nKey::EditorJsonInvalidRecords,
                &[("count", &invalid.to_string())],
            )
            .into()
        })
    }

    /// 预算失败或整份 JSON 无法解析时的可见说明；JSONL 的逐行错误不挡住
    /// 其他记录，由行错误与计数单独显示。
    fn fatal_error(
        &self,
        failure: Option<&Self::Failure>,
        projection: Option<&Self::Projection>,
    ) -> Option<(SharedString, SharedString)> {
        if let Some(failure) = failure {
            let message = match failure {
                ProjectionFailure::Cancelled => return None,
                ProjectionFailure::InputTooLarge => t_fmt(
                    L10nKey::EditorJsonInputLimit,
                    &[("mib", &(STRUCTURE_INPUT_BYTES / (1024 * 1024)).to_string())],
                )
                .into(),
                ProjectionFailure::TooManyNodes => t_fmt(
                    L10nKey::EditorJsonNodeLimit,
                    &[("count", &STRUCTURE_NODES.to_string())],
                )
                .into(),
                ProjectionFailure::TooManyRecords => t_fmt(
                    L10nKey::EditorJsonRecordLimit,
                    &[("count", &STRUCTURE_RECORDS.to_string())],
                )
                .into(),
                ProjectionFailure::PathsTooLarge => t_fmt(
                    L10nKey::EditorJsonPathLimit,
                    &[("mib", &(STRUCTURE_PATH_BYTES / (1024 * 1024)).to_string())],
                )
                .into(),
            };
            return Some((t(L10nKey::EditorJsonStructureUnavailable).into(), message));
        }
        let projection = projection?;
        let error = projection.error()?;
        Some((
            t(L10nKey::EditorJsonInvalid).into(),
            t_fmt(
                L10nKey::EditorJsonInvalidDetail,
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
            detail: node.detail_range(),
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
            JsonNodeKind::String => theme.green,
            JsonNodeKind::Number => theme.blue,
            JsonNodeKind::Boolean => theme.magenta,
            JsonNodeKind::InvalidRecord => theme.warning,
            JsonNodeKind::Object | JsonNodeKind::Array | JsonNodeKind::Records => {
                theme.muted_foreground
            }
            JsonNodeKind::Null => theme.muted_foreground,
        };
        Some(NodeRow {
            key: node.key.clone().into(),
            value: node.value.clone().into(),
            color,
            badge: None,
            record_line: node.record_line,
            error: node.error.clone().map(SharedString::from),
            aria_label: node.path.clone().into(),
        })
    }

    /// 打开时自动展开根的直接子容器，复现上游的浅层可读树。
    fn default_expansion(&self, projection: &Self::Projection, expanded: &mut [bool]) {
        let Some(root) = projection.root() else {
            return;
        };
        if root >= expanded.len() {
            return;
        }
        expanded[root] = true;
        for &child in projection.children(root) {
            if projection.is_expandable(child)
                && projection.children(child).len() <= AUTO_EXPAND_CHILDREN
            {
                expanded[child] = true;
            }
        }
    }

    fn selection_restore(&self) -> SelectionRestore {
        SelectionRestore::PathFallback
    }

    fn edit_buttons(
        &self,
        projection: &Self::Projection,
        selected: Option<usize>,
    ) -> Vec<EditButton<Self::EditRequest>> {
        let is_lines = self.format == JsonFormat::JsonLines;
        let mut caps_value = false;
        let mut caps_key = false;
        let mut caps_add = false;
        let mut caps_add_is_array = false;
        let mut caps_remove = false;
        let mut record_root = false;
        let mut caps_remove_record = false;
        if let Some(index) = selected {
            if let Some(node) = projection.nodes().get(index) {
                record_root = projection.is_record_root(index);
                caps_value = record_root || value_editable(node);
                caps_key = node.key_range.is_some();
                caps_add = matches!(node.kind, JsonNodeKind::Object | JsonNodeKind::Array);
                caps_add_is_array = node.kind == JsonNodeKind::Array;
                caps_remove = node.parent.is_some_and(|parent| {
                    node.key_range.is_some()
                        || projection
                            .nodes()
                            .get(parent)
                            .is_some_and(|parent| parent.kind == JsonNodeKind::Array)
                });
                caps_remove_record = record_root;
            }
        }
        let mut buttons = vec![
            EditButton {
                id: "edit-value",
                label: match record_root {
                    true => t(L10nKey::EditorJsonEditRecord).into(),
                    false => t(L10nKey::EditorJsonEditValue).into(),
                },
                enabled: caps_value,
                flow: EditFlow::Panel(JsonEditRequest::Value),
            },
            EditButton {
                id: "rename-key",
                label: t(L10nKey::EditorJsonRenameKey).into(),
                enabled: caps_key,
                flow: EditFlow::Panel(JsonEditRequest::Key),
            },
            EditButton {
                id: "add-member",
                label: match caps_add_is_array {
                    true => t(L10nKey::EditorJsonAddItem).into(),
                    false => t(L10nKey::EditorJsonAddKey).into(),
                },
                enabled: caps_add,
                flow: EditFlow::Panel(JsonEditRequest::AddMember),
            },
            EditButton {
                id: "remove-member",
                label: t(L10nKey::EditorJsonRemove).into(),
                enabled: caps_remove,
                flow: EditFlow::Immediate(JsonEditRequest::RemoveMember),
            },
        ];
        if is_lines {
            buttons.push(EditButton {
                id: "add-record",
                label: t(L10nKey::EditorJsonAddRecord).into(),
                enabled: true,
                flow: EditFlow::Panel(JsonEditRequest::AddRecord),
            });
            buttons.push(EditButton {
                id: "remove-record",
                label: t(L10nKey::EditorJsonRemoveRecord).into(),
                enabled: caps_remove_record,
                flow: EditFlow::Immediate(JsonEditRequest::RemoveRecord),
            });
        }
        buttons
    }

    fn describe_edit(
        &self,
        request: &Self::EditRequest,
        projection: &Self::Projection,
        text: &str,
        selected: Option<usize>,
    ) -> Option<EditPlan<Self::Target>> {
        match request {
            JsonEditRequest::Value => {
                let index = selected?;
                let node = projection.nodes().get(index)?;
                let record_root = projection.is_record_root(index);
                if !record_root && !value_editable(node) {
                    return None;
                }
                let raw = text.get(node.range.clone())?;
                let title_key = match record_root {
                    true => L10nKey::EditorJsonEditRecordTitle,
                    false => L10nKey::EditorJsonEditValueTitle,
                };
                Some(EditPlan::Panel(EditPanelDesc {
                    title: t_fmt(title_key, &[("path", &node.path)]).into(),
                    key: None,
                    value: EditField::new(
                        t(L10nKey::EditorJsonValueName),
                        t(L10nKey::EditorJsonValuePlaceholder),
                        raw,
                    ),
                    target: Some(NodeTarget::of(node)),
                    notice: None,
                    multiline: false,
                }))
            }
            JsonEditRequest::Key => {
                let node = selected.and_then(|index| projection.nodes().get(index))?;
                let key_text = node.key_text.clone()?;
                node.key_range.as_ref()?;
                Some(EditPlan::Panel(EditPanelDesc {
                    title: t_fmt(L10nKey::EditorJsonRenameKeyTitle, &[("path", &node.path)]).into(),
                    key: None,
                    value: EditField::new(
                        t(L10nKey::EditorJsonKeyName),
                        t(L10nKey::EditorJsonKeyPlaceholder),
                        key_text,
                    ),
                    target: Some(NodeTarget::of(node)),
                    notice: None,
                    multiline: false,
                }))
            }
            JsonEditRequest::AddMember => {
                let node = selected.and_then(|index| projection.nodes().get(index))?;
                let (title_key, key_field) = match node.kind {
                    JsonNodeKind::Object => (
                        L10nKey::EditorJsonAddKeyTitle,
                        Some(EditField::new(
                            t(L10nKey::EditorJsonKeyName),
                            t(L10nKey::EditorJsonKeyPlaceholder),
                            "",
                        )),
                    ),
                    JsonNodeKind::Array => (L10nKey::EditorJsonAddItemTitle, None),
                    _ => return None,
                };
                Some(EditPlan::Panel(EditPanelDesc {
                    title: t_fmt(title_key, &[("path", &node.path)]).into(),
                    key: key_field,
                    value: EditField::new(
                        t(L10nKey::EditorJsonValueName),
                        t(L10nKey::EditorJsonValuePlaceholder),
                        "",
                    ),
                    target: Some(NodeTarget::of(node)),
                    notice: None,
                    multiline: false,
                }))
            }
            // JSONL／NDJSON 的记录动作：选中一条记录时在其后插入，否则追加到
            // 文件末尾；新记录必须保持单行。
            JsonEditRequest::AddRecord => {
                if self.format != JsonFormat::JsonLines {
                    return None;
                }
                let record = selected.filter(|index| projection.is_record_root(*index));
                let after = record.map(|index| NodeTarget::of(&projection.nodes()[index]));
                let line = record.and_then(|index| projection.nodes()[index].record_line);
                let title = match line {
                    Some(line) => t_fmt(
                        L10nKey::EditorJsonAddRecordAfterTitle,
                        &[("line", &line.to_string())],
                    )
                    .into(),
                    None => t(L10nKey::EditorJsonAddRecordEndTitle).into(),
                };
                Some(EditPlan::Panel(EditPanelDesc {
                    title,
                    key: None,
                    value: EditField::new(
                        t(L10nKey::EditorJsonValueName),
                        t(L10nKey::EditorJsonValuePlaceholder),
                        "",
                    ),
                    target: after,
                    notice: None,
                    multiline: false,
                }))
            }
            JsonEditRequest::RemoveMember => {
                let node = selected.and_then(|index| projection.nodes().get(index))?;
                Some(EditPlan::Immediate {
                    target: Some(NodeTarget::of(node)),
                })
            }
            JsonEditRequest::RemoveRecord => {
                if self.format != JsonFormat::JsonLines {
                    return None;
                }
                let index = selected.filter(|index| projection.is_record_root(*index))?;
                Some(EditPlan::Immediate {
                    target: Some(NodeTarget::of(&projection.nodes()[index])),
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
        match (request, target) {
            (JsonEditRequest::Value, Some(target)) => Ok(Mutation::SetValue {
                target: target.clone(),
                value,
            }),
            (JsonEditRequest::Key, Some(target)) => Ok(Mutation::RenameKey {
                target: target.clone(),
                key: value,
            }),
            (JsonEditRequest::AddMember, Some(target)) => Ok(Mutation::AddMember {
                container: target.clone(),
                key,
                value,
            }),
            (JsonEditRequest::AddRecord, after) => Ok(Mutation::AddRecord {
                after: after.clone(),
                value,
            }),
            (JsonEditRequest::RemoveMember, Some(target)) => Ok(Mutation::RemoveMember {
                target: target.clone(),
            }),
            (JsonEditRequest::RemoveRecord, Some(target)) => Ok(Mutation::RemoveRecord {
                target: target.clone(),
            }),
            _ => Err(MutationError::Unsupported),
        }
    }

    fn plan_mutation(
        &self,
        text: &str,
        projection: &Self::Projection,
        mutation: &Self::Mutation,
    ) -> Result<Vec<(Range<usize>, String)>, Self::MutationError> {
        mutation::plan(text, projection, self.format, mutation)
    }

    fn mutation_message(
        &self,
        error: StructuredMutationError<Self::MutationError>,
    ) -> SharedString {
        match error {
            StructuredMutationError::Stale => t(L10nKey::EditorJsonEditStale).into(),
            StructuredMutationError::Plan(MutationError::Stale) => {
                t(L10nKey::EditorJsonEditStale).into()
            }
            StructuredMutationError::Plan(MutationError::Unsupported) => {
                t(L10nKey::EditorJsonEditUnsupported).into()
            }
            StructuredMutationError::Plan(MutationError::InvalidValue(message)) => t_fmt(
                L10nKey::EditorJsonEditInvalidValue,
                &[("message", &message)],
            )
            .into(),
            StructuredMutationError::Plan(MutationError::RecordNotSingleLine) => {
                t(L10nKey::EditorJsonEditRecordLine).into()
            }
            StructuredMutationError::Plan(MutationError::ResultInvalid {
                line,
                column,
                message,
            }) => t_fmt(
                L10nKey::EditorJsonEditResultInvalid,
                &[
                    ("line", &line.to_string()),
                    ("column", &column.to_string()),
                    ("message", &message),
                ],
            )
            .into(),
        }
    }

    fn loading_label(&self) -> SharedString {
        t(L10nKey::EditorJsonParsing).into()
    }

    fn unavailable(&self) -> (SharedString, SharedString) {
        (
            t(L10nKey::EditorJsonStructureUnavailable).into(),
            t(L10nKey::EditorJsonSourceStaysEditable).into(),
        )
    }

    fn full_value_label(&self) -> SharedString {
        t(L10nKey::EditorJsonFullValue).into()
    }

    fn copy_value_label(&self) -> SharedString {
        t(L10nKey::EditorJsonCopyValue).into()
    }

    fn reveal_label(&self) -> SharedString {
        t(L10nKey::EditorJsonReveal).into()
    }
}

fn value_editable(node: &projection::JsonNode) -> bool {
    matches!(
        node.kind,
        JsonNodeKind::String | JsonNodeKind::Number | JsonNodeKind::Boolean | JsonNodeKind::Null
    ) && !node.range.is_empty()
}

/// 一份 JSON 文件的内容：共享 Document 与按最新 revision 安装的投影。
/// 具体构造见 [`JsonShell::new`]。
pub type JsonDocument = StructuredDocument<JsonShell>;

/// 一个 presentation 里的 JSON 呈现：模式、源码选择、结构展开与滚动。
pub type JsonView = StructuredView<JsonShell>;

/// JSON 呈现模式：结构树或源码。
pub type JsonMode = StructuredMode;

/// 结构预览持有键盘时成立的上下文；源码面不声明它，文字编辑键不会被截走。
pub const KEY_CONTEXT: &str = <JsonShell as StructuredViewFormat>::KEY_CONTEXT;

/// 右侧菜单的 JSON 项：默认只给结构与源码切换；结构和源码切换之外的编辑
/// 命令仍由普通文本菜单提供。
pub fn menu_entries(
    menu: gpui_kit::component::menu::PopupMenu,
) -> gpui_kit::component::menu::PopupMenu {
    super::structured_view::menu_entries(menu, L10nKey::EditorJsonToggleStructure)
}

/// 解析的确定性单元测试门闩，不编译进产品或独立 native 消费者。
#[cfg(any(test, feature = "test-support"))]
pub use super::structured_document::test_support;
