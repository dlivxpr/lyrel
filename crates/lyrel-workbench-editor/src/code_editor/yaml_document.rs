//! YAML 文档：一份共享逐行保真源码，加上按 revision 派生的无损结构树。
//!
//! 源码始终是同一 [`lyrel_markdown_editor::Document`] 的 plain-source
//! presentation。anchors／aliases／tags／comments 与多文档都是真实语义，
//! 不展开成副本，也不通过 JSON／YamlValue 回写。文档编排与 presentation 壳
//! 都在 `structured_document`／`structured_view`；这个模块只提供 YAML 的格式
//! 壳：解析、行呈现（anchor／alias／tag 徽标）、多文档计数、块标量与复杂键的
//! 源码 handoff、锚点影响提示与 mutation 规划。无损编辑由 `lyrel-document`
//! 的 YAML 引擎规划成局部 splice，经共享 `Document::replace_ranges` 成为一条
//! 普通 history；写入前按 revision 与整篇解析复核。

use std::ops::Range;

use gpui_kit::SharedString;
use gpui_kit::component::Theme;
use lyrel_document::state::CancelToken;
use lyrel_document::tree::Treelike;
use lyrel_document::yaml::mutation::{self, Mutation, MutationError, NodeTarget, UnsupportedEdit};
use lyrel_document::yaml::projection::{
    self, ContainerKind, ProjectionFailure, STRUCTURE_DEPTH, STRUCTURE_INPUT_BYTES,
    STRUCTURE_NODES, STRUCTURE_PATH_BYTES, YamlNode, YamlNodeKind, YamlProjection, YamlScalarKind,
};

use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};

use super::structure_edit::EditField;
use super::structured_document::{StructuredDocument, StructuredDocumentFormat};
use super::structured_view::{
    AUTO_EXPAND_CHILDREN, EditButton, EditFlow, EditPanelDesc, EditPlan, NodeIdentity, NodeRow,
    StructuredMode, StructuredMutationError, StructuredView, StructuredViewFormat,
};

/// YAML 格式壳：单位结构文档/视图适配器；所有编排与呈现状态都属于共享模块。
#[derive(Clone, Copy, Debug)]
pub struct YamlShell;

impl StructuredDocumentFormat for YamlShell {
    type Projection = YamlProjection;
    type Failure = ProjectionFailure;

    fn parse(&self, text: &str, cancel: &CancelToken) -> Result<Self::Projection, Self::Failure> {
        projection::project(text, cancel)
    }
}

/// 一个工具栏树命令：打开面板时固定目标 occurrence 与源码 revision，
/// 提交时两者都必须仍匹配。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YamlEditRequest {
    Value,
    Key,
    AddMember,
    AddItem,
    Remove,
}

impl StructuredViewFormat for YamlShell {
    type EditRequest = YamlEditRequest;
    type Mutation = Mutation;
    type MutationError = MutationError;
    type Target = NodeTarget;

    const KEY_CONTEXT: &str = "YamlStructure";
    const STRUCTURE_UNDO_REDO: bool = true;
    /// YAML 没有完整值面板：块标量与复杂键的保真靠双击直接 reveal 源码。
    const FULL_VALUE: bool = false;

    fn element_prefix(&self) -> &'static str {
        "yaml"
    }

    fn format_label(&self) -> SharedString {
        "YAML".into()
    }

    fn mode_labels(&self, mode: StructuredMode) -> (SharedString, SharedString) {
        match mode {
            StructuredMode::Structure => (
                t(L10nKey::EditorYamlSource).into(),
                t(L10nKey::EditorYamlShowSource).into(),
            ),
            StructuredMode::Source => (
                t(L10nKey::EditorYamlStructure).into(),
                t(L10nKey::EditorYamlShowStructure).into(),
            ),
        }
    }

    fn node_count(&self, projection: &Self::Projection) -> usize {
        projection.node_count()
    }

    fn display_node_count(&self, projection: &Self::Projection) -> Option<usize> {
        Some(projection.node_count())
    }

    fn documents_notice(&self, projection: &Self::Projection) -> Option<SharedString> {
        Some(
            t_fmt(
                L10nKey::EditorYamlDocuments,
                &[("count", &projection.document_count().to_string())],
            )
            .into(),
        )
    }

    fn count_notice(&self, projection: &Self::Projection) -> Option<SharedString> {
        self.display_node_count(projection)
            .map(|count| t_fmt(L10nKey::EditorYamlNodes, &[("count", &count.to_string())]).into())
    }

    /// 预算失败或整份 YAML 无法解析时的可见说明。
    fn fatal_error(
        &self,
        failure: Option<&Self::Failure>,
        projection: Option<&Self::Projection>,
    ) -> Option<(SharedString, SharedString)> {
        if let Some(failure) = failure {
            let message = match failure {
                ProjectionFailure::Cancelled => return None,
                ProjectionFailure::InputTooLarge => t_fmt(
                    L10nKey::EditorYamlInputLimit,
                    &[("mib", &(STRUCTURE_INPUT_BYTES / (1024 * 1024)).to_string())],
                )
                .into(),
                ProjectionFailure::TooManyNodes => t_fmt(
                    L10nKey::EditorYamlNodeLimit,
                    &[("count", &STRUCTURE_NODES.to_string())],
                )
                .into(),
                ProjectionFailure::PathsTooLarge => t_fmt(
                    L10nKey::EditorYamlPathLimit,
                    &[("mib", &(STRUCTURE_PATH_BYTES / (1024 * 1024)).to_string())],
                )
                .into(),
                ProjectionFailure::TooDeep => t_fmt(
                    L10nKey::EditorYamlDepthLimit,
                    &[("count", &STRUCTURE_DEPTH.to_string())],
                )
                .into(),
            };
            return Some((t(L10nKey::EditorYamlStructureUnavailable).into(), message));
        }
        let projection = projection?;
        let error = projection.error()?;
        Some((
            t(L10nKey::EditorYamlInvalid).into(),
            t_fmt(
                L10nKey::EditorYamlInvalidDetail,
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
            YamlNodeKind::Scalar(YamlScalarKind::String) => theme.green,
            YamlNodeKind::Scalar(YamlScalarKind::Integer | YamlScalarKind::Float) => theme.blue,
            YamlNodeKind::Scalar(YamlScalarKind::Boolean) => theme.magenta,
            YamlNodeKind::Scalar(
                YamlScalarKind::Null | YamlScalarKind::Timestamp | YamlScalarKind::Other,
            ) => theme.muted_foreground,
            YamlNodeKind::Alias => theme.yellow,
            YamlNodeKind::Tagged => theme.magenta,
            YamlNodeKind::File
            | YamlNodeKind::Mapping
            | YamlNodeKind::Sequence
            | YamlNodeKind::Empty => theme.muted_foreground,
        };
        Some(NodeRow {
            key: node.key.clone().into(),
            value: node.value.clone().into(),
            color,
            badge: badge_for(node, projection),
            record_line: None,
            error: node.alias_error.clone().map(SharedString::from),
            aria_label: node.path.clone().into(),
        })
    }

    /// 打开时自动展开根的两层直接子容器，复现上游的浅层可读树。
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
                for &grandchild in projection.children(child) {
                    if projection.is_expandable(grandchild)
                        && projection.children(grandchild).len() <= AUTO_EXPAND_CHILDREN
                    {
                        expanded[grandchild] = true;
                    }
                }
            }
        }
    }

    fn edit_buttons(
        &self,
        projection: &Self::Projection,
        selected: Option<usize>,
    ) -> Vec<EditButton<Self::EditRequest>> {
        let mut caps_value = false;
        let mut caps_key = false;
        let mut caps_add = false;
        let mut caps_add_is_sequence = false;
        let mut caps_remove = false;
        if let Some(node) = selected.and_then(|index| projection.nodes().get(index)) {
            caps_value = node.value_editable() || node.block_scalar;
            caps_key =
                node.rename_key_editable() || (node.entry && node.key_detail_range.is_some());
            caps_add = node.can_add();
            caps_add_is_sequence = node.container == Some(ContainerKind::Sequence);
            caps_remove = node.can_remove();
        }
        let add_request = match caps_add_is_sequence {
            true => YamlEditRequest::AddItem,
            false => YamlEditRequest::AddMember,
        };
        vec![
            EditButton {
                id: "edit-value",
                label: t(L10nKey::EditorJsonEditValue).into(),
                enabled: caps_value,
                flow: EditFlow::Panel(YamlEditRequest::Value),
            },
            EditButton {
                id: "rename-key",
                label: t(L10nKey::EditorJsonRenameKey).into(),
                enabled: caps_key,
                flow: EditFlow::Panel(YamlEditRequest::Key),
            },
            EditButton {
                id: "add-member",
                label: match caps_add_is_sequence {
                    true => t(L10nKey::EditorJsonAddItem).into(),
                    false => t(L10nKey::EditorJsonAddKey).into(),
                },
                enabled: caps_add,
                flow: EditFlow::Panel(add_request),
            },
            EditButton {
                id: "remove-member",
                label: t(L10nKey::EditorJsonRemove).into(),
                enabled: caps_remove,
                flow: EditFlow::Immediate(YamlEditRequest::Remove),
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
            // 块标量用多行面板编辑原始片段（header+正文），按一个块标量校验后
            // 局部 splice；普通标量取 value_range 的原文拼写。
            YamlEditRequest::Value => {
                let node = selected.and_then(|index| projection.nodes().get(index))?;
                let (raw, multiline) = match () {
                    _ if node.block_scalar => (text.get(node.content_range.clone())?, true),
                    _ if node.value_editable() => {
                        let range = node.value_range.clone()?;
                        (text.get(range)?, false)
                    }
                    _ => return None,
                };
                Some(EditPlan::Panel(EditPanelDesc {
                    title: t_fmt(L10nKey::EditorYamlEditValueTitle, &[("path", &node.path)]).into(),
                    key: None,
                    value: EditField::new(
                        t(L10nKey::EditorYamlValueName),
                        t(L10nKey::EditorYamlValuePlaceholder),
                        raw,
                    ),
                    target: NodeTarget::of(node),
                    notice: panel_anchor_notice(node, false, projection),
                    multiline,
                }))
            }
            // 复杂键（flow collection 键）是完整局部片段，按一个 mapping 键校验；
            // 多行键不猜测重排，handoff 到 Source 的精确范围。
            YamlEditRequest::Key => {
                let node = selected.and_then(|index| projection.nodes().get(index))?;
                if node.rename_key_editable() {
                    let raw = node.key_text.clone()?;
                    return Some(key_panel(node, &raw, projection));
                }
                if !(node.entry && node.key_detail_range.is_some()) {
                    return None;
                }
                let range = node.key_detail_range.clone()?;
                let raw = text.get(range.clone())?;
                if raw.contains(['\n', '\r']) {
                    return Some(EditPlan::Handoff {
                        range,
                        notice: handoff_notice(node, projection),
                    });
                }
                Some(key_panel(node, raw, projection))
            }
            YamlEditRequest::AddMember => {
                let node = selected.and_then(|index| projection.nodes().get(index))?;
                (node.container == Some(ContainerKind::Mapping))
                    .then(|| add_panel(node, L10nKey::EditorYamlAddKeyTitle, true, projection))
            }
            YamlEditRequest::AddItem => {
                let node = selected.and_then(|index| projection.nodes().get(index))?;
                (node.container == Some(ContainerKind::Sequence))
                    .then(|| add_panel(node, L10nKey::EditorYamlAddItemTitle, false, projection))
            }
            YamlEditRequest::Remove => {
                let node = selected.and_then(|index| projection.nodes().get(index))?;
                Some(EditPlan::Immediate {
                    target: NodeTarget::of(node),
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
            YamlEditRequest::Value => Mutation::SetValue {
                target: target.clone(),
                value,
            },
            YamlEditRequest::Key => Mutation::RenameKey {
                target: target.clone(),
                key: value,
            },
            YamlEditRequest::AddMember => Mutation::AddMember {
                container: target.clone(),
                key: key.unwrap_or_default(),
                value,
            },
            YamlEditRequest::AddItem => Mutation::AddItem {
                container: target.clone(),
                value,
            },
            YamlEditRequest::Remove => Mutation::Remove {
                target: target.clone(),
            },
        })
    }

    /// 命令成功后的锚点影响说明（写入前用当前投影计算）。
    fn applied_notice(
        &self,
        request: &Self::EditRequest,
        target: &Self::Target,
        projection: &Self::Projection,
    ) -> Option<SharedString> {
        let key = matches!(request, YamlEditRequest::Key);
        let node = projection
            .nodes()
            .iter()
            .find(|node| node.path == target.path && node.range == target.range)?;
        let (name, count) = effective_anchor(node, key, projection)?;
        Some(anchor_edited(&name, count))
    }

    fn plan_mutation(
        &self,
        text: &str,
        projection: &Self::Projection,
        mutation: &Self::Mutation,
    ) -> Result<Vec<(Range<usize>, String)>, Self::MutationError> {
        mutation::plan(text, projection, mutation)
    }

    fn mutation_message(
        &self,
        error: StructuredMutationError<Self::MutationError>,
    ) -> SharedString {
        match error {
            StructuredMutationError::Stale => t(L10nKey::EditorYamlEditStale).into(),
            StructuredMutationError::Plan(MutationError::Stale) => {
                t(L10nKey::EditorYamlEditStale).into()
            }
            StructuredMutationError::Plan(MutationError::Unsupported(edit)) => {
                let key = match edit {
                    UnsupportedEdit::Alias => L10nKey::EditorYamlEditAlias,
                    UnsupportedEdit::MissingValue => L10nKey::EditorYamlEditMissingValue,
                    UnsupportedEdit::MultiLine => L10nKey::EditorYamlEditMultiLine,
                    UnsupportedEdit::NotScalar => L10nKey::EditorYamlEditNotScalar,
                    UnsupportedEdit::EmptyDocument => L10nKey::EditorYamlEditEmptyDocument,
                    UnsupportedEdit::NotContainer => L10nKey::EditorYamlEditUnsupported,
                };
                t(key).into()
            }
            StructuredMutationError::Plan(MutationError::InvalidFragment(message)) => t_fmt(
                L10nKey::EditorYamlEditInvalidValue,
                &[("message", &message)],
            )
            .into(),
            StructuredMutationError::Plan(MutationError::ResultInvalid {
                line,
                column,
                message,
            }) => t_fmt(
                L10nKey::EditorYamlEditResultInvalid,
                &[
                    ("line", &line.to_string()),
                    ("column", &column.to_string()),
                    ("message", &message),
                ],
            )
            .into(),
            StructuredMutationError::Plan(MutationError::AliasesReference { name, count }) => {
                t_fmt(
                    L10nKey::EditorYamlEditAliasesReference,
                    &[
                        ("name", &name.unwrap_or_else(|| "?".to_owned())),
                        ("count", &count.to_string()),
                    ],
                )
                .into()
            }
        }
    }

    fn loading_label(&self) -> SharedString {
        t(L10nKey::EditorYamlParsing).into()
    }

    fn unavailable(&self) -> (SharedString, SharedString) {
        (
            t(L10nKey::EditorYamlStructureUnavailable).into(),
            t(L10nKey::EditorYamlSourceStaysEditable).into(),
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

/// 追加成员面板：mapping 带键字段，sequence 只要值。
fn add_panel(
    node: &YamlNode,
    title_key: L10nKey,
    with_key: bool,
    projection: &YamlProjection,
) -> EditPlan<NodeTarget> {
    EditPlan::Panel(EditPanelDesc {
        title: t_fmt(title_key, &[("path", &node.path)]).into(),
        key: with_key.then(|| {
            EditField::new(
                t(L10nKey::EditorYamlKeyName),
                t(L10nKey::EditorYamlKeyPlaceholder),
                "",
            )
        }),
        value: EditField::new(
            t(L10nKey::EditorYamlValueName),
            t(L10nKey::EditorYamlValuePlaceholder),
            "",
        ),
        target: NodeTarget::of(node),
        notice: panel_anchor_notice(node, false, projection),
        multiline: false,
    })
}

/// 改名键面板的共享描述：值字段预填键的原文（可编辑键文本或复杂键片段）。
fn key_panel(node: &YamlNode, raw: &str, projection: &YamlProjection) -> EditPlan<NodeTarget> {
    EditPlan::Panel(EditPanelDesc {
        title: t_fmt(L10nKey::EditorYamlRenameKeyTitle, &[("path", &node.path)]).into(),
        key: None,
        value: EditField::new(
            t(L10nKey::EditorYamlKeyName),
            t(L10nKey::EditorYamlKeyPlaceholder),
            raw,
        ),
        target: NodeTarget::of(node),
        notice: panel_anchor_notice(node, true, projection),
        multiline: false,
    })
}

/// 行的徽标：键锚点/键 tag 在前，值锚点/tag 居中，alias 解析说明殿后。
fn badge_for(node: &YamlNode, projection: &YamlProjection) -> Option<SharedString> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(anchor) = &node.key_anchor {
        parts.push(format!("&{anchor} (key)"));
    }
    if let Some(tag) = &node.key_tag {
        parts.push(tag.clone());
    }
    if let Some(anchor) = &node.anchor {
        parts.push(format!("&{anchor}"));
    }
    if let Some(tag) = &node.tag {
        parts.push(tag.clone());
    }
    if let Some(alias) = &node.alias {
        match (&node.alias_error, node.alias_target) {
            (Some(_), _) => parts.push(format!("*{alias} → missing")),
            (_, Some(target)) => {
                let anchor = projection
                    .nodes()
                    .get(target)
                    .and_then(|target| target.anchor.as_deref())
                    .unwrap_or("?");
                parts.push(format!("*{alias} → &{anchor}"));
            }
            _ => parts.push(format!("*{alias}")),
        }
    }
    match parts.is_empty() {
        true => None,
        false => Some(parts.join(" · ").into()),
    }
}

/// 选中节点自身或最近的带锚点祖先。编辑锚定 collection 的后代同样会
/// 改变别名解析结果，因此影响说明不能只看直接值。
fn effective_anchor(
    node: &YamlNode,
    key: bool,
    projection: &YamlProjection,
) -> Option<(String, usize)> {
    if key {
        if let Some(anchor) = &node.key_anchor {
            return Some((anchor.clone(), node.key_alias_refs));
        }
    } else if let Some(anchor) = &node.anchor {
        return Some((anchor.clone(), node.alias_refs));
    }
    let mut parent = node.parent;
    while let Some(index) = parent {
        let ancestor = projection.nodes().get(index)?;
        if let Some(anchor) = &ancestor.anchor {
            return Some((anchor.clone(), ancestor.alias_refs));
        }
        parent = ancestor.parent;
    }
    None
}

/// 面板里的锚点影响提示；无锚点则为 None。
fn panel_anchor_notice(
    node: &YamlNode,
    key: bool,
    projection: &YamlProjection,
) -> Option<SharedString> {
    let (name, count) = effective_anchor(node, key, projection)?;
    Some(anchor_notice(&name, count, key))
}

/// 编辑面板打开时的锚点影响说明：指出受影响的是键还是值。
fn anchor_notice(anchor: &str, aliases: usize, key: bool) -> SharedString {
    let where_ = if key {
        t(L10nKey::EditorYamlKeyName)
    } else {
        t(L10nKey::EditorYamlValueName)
    };
    t_fmt(
        match aliases {
            0 => L10nKey::EditorYamlAnchorImpactNone,
            _ => L10nKey::EditorYamlAnchorImpact,
        },
        &[
            ("anchor", anchor),
            ("count", &aliases.to_string()),
            ("field", where_),
        ],
    )
    .into()
}

/// 命令成功后的锚点编辑说明（工具栏 `yaml-notice`）。
fn anchor_edited(name: &str, count: usize) -> SharedString {
    t_fmt(
        match count {
            0 => L10nKey::EditorYamlAnchorEditedNone,
            _ => L10nKey::EditorYamlAnchorEdited,
        },
        &[("anchor", name), ("count", &count.to_string())],
    )
    .into()
}

/// 复杂键 handoff 到 Source 的说明：基础文案加上（可选的）键锚点影响。
fn handoff_notice(node: &YamlNode, projection: &YamlProjection) -> SharedString {
    let mut notice = t(L10nKey::EditorYamlComplexKeySource).to_owned();
    if let Some((anchor, count)) = effective_anchor(node, true, projection) {
        notice.push(' ');
        notice.push_str(&anchor_notice(&anchor, count, true));
    }
    notice.into()
}

/// 一份 YAML 文件的内容：共享 Document 与按最新 revision 安装的投影。
pub type YamlDocument = StructuredDocument<YamlShell>;

/// 一个 presentation 里的 YAML 呈现：模式、源码选择、结构展开与滚动。
pub type YamlView = StructuredView<YamlShell>;

/// YAML 呈现模式：结构树或源码。
pub type YamlMode = StructuredMode;

/// 结构预览持有键盘时成立的上下文；源码面不声明它，文字编辑键不会被截走。
pub const KEY_CONTEXT: &str = <YamlShell as StructuredViewFormat>::KEY_CONTEXT;

/// 右侧菜单的 YAML 项：默认只给结构与源码切换；编辑命令由结构工具栏提供。
pub fn menu_entries(
    menu: gpui_kit::component::menu::PopupMenu,
) -> gpui_kit::component::menu::PopupMenu {
    super::structured_view::menu_entries(menu, L10nKey::EditorYamlToggleStructure)
}

impl StructuredView<YamlShell> {
    /// 多文档计数（状态栏与工具栏的次计数）。
    pub fn document_count(&self, cx: &gpui_kit::App) -> Option<usize> {
        self.document()
            .read(cx)
            .projection()
            .map(YamlProjection::document_count)
    }

    /// 成功应用命令或源码 handoff 后的提示（锚点编辑说明、复杂键移交说明）。
    #[cfg(any(test, feature = "test-support"))]
    pub fn mutation_notice(&self) -> Option<SharedString> {
        self.notice()
    }
}

/// 解析的确定性单元测试门闩，不编译进产品或独立 native 消费者。
#[cfg(any(test, feature = "test-support"))]
pub use super::structured_document::test_support;
