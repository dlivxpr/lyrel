//! XML 文档：一份共享逐行保真源码，加上按 revision 派生的有序结构视图。
//!
//! 源码始终是同一 [`lyrel_markdown_editor::Document`] 的 plain-source
//! presentation；保存、撤销、reload 与关闭都走既有工作台生命周期。结构节点
//! 只保存源码范围与显示文本，不复制第二份可变内容，也不把 XML 转成通用对象。
//!
//! 文档编排（`Document` 订阅、debounce 单飞后台解析、取消与 revision 守卫）
//! 与 presentation 壳（模式切换、展开导航、滚动同步、源码定位、完整值与
//! 结构编辑会话）都在 `structured_document`／`structured_view`；这个模块只
//! 提供 XML 的格式壳：解析、行呈现（命名空间／CDATA／注释／PI 徽标）、元素/
//! 属性计数、多行值编辑与 mutation 规划。元素、属性、文本、CDATA、注释与 PI
//! 按源码顺序显示；混合内容不会被压成一个值。属性引号风格、命名空间声明、
//! 自闭合标签与精确范围由 `lyrel-document` 的 XML 引擎在局部 splice 里保留，
//! 经共享 `Document::replace_ranges` 成为一条普通 history。

use std::ops::Range;

use gpui_kit::SharedString;
use gpui_kit::component::Theme;
use lyrel_document::state::CancelToken;
use lyrel_document::tree::{self, Treelike};
use lyrel_document::xml::mutation::{self, Mutation, MutationError, NodeTarget};
use lyrel_document::xml::projection::{
    self, ProjectionFailure, XML_INPUT_BYTES, XML_NODES, XML_PATH_BYTES, XmlNodeKind, XmlProjection,
};

use lyrel_ui_base::i18n::{L10nKey, t, t_fmt};

use super::structure_edit::EditField;
use super::structured_document::{StructuredDocument, StructuredDocumentFormat};
use super::structured_view::{
    EditButton, EditFlow, EditPanelDesc, EditPlan, NodeIdentity, NodeRow, StructuredMode,
    StructuredMutationError, StructuredView, StructuredViewFormat,
};

/// XML 格式壳：单位结构文档/视图适配器；所有编排与呈现状态都属于共享模块。
#[derive(Clone, Copy, Debug)]
pub struct XmlShell;

impl StructuredDocumentFormat for XmlShell {
    type Projection = XmlProjection;
    type Failure = ProjectionFailure;

    fn parse(&self, text: &str, cancel: &CancelToken) -> Result<Self::Projection, Self::Failure> {
        projection::project(text, cancel)
    }
}

/// 一个工具栏树命令：目标 occurrence 在打开面板时固定，提交时按当前
/// revision 重新解析；面板只是输入控件，不持有文档。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XmlEditRequest {
    Value,
    Rename,
    AddAttribute,
    AddChild,
    AddText,
    Remove,
}

impl StructuredViewFormat for XmlShell {
    type EditRequest = XmlEditRequest;
    type Mutation = Mutation;
    type MutationError = MutationError;
    type Target = NodeTarget;

    const KEY_CONTEXT: &str = "XmlStructure";
    const REOPEN_ROOT_ON_REFRESH: bool = false;

    fn element_prefix(&self) -> &'static str {
        "xml"
    }

    fn format_label(&self) -> SharedString {
        "XML".into()
    }

    fn mode_labels(&self, mode: StructuredMode) -> (SharedString, SharedString) {
        match mode {
            StructuredMode::Structure => (
                t(L10nKey::EditorXmlSource).into(),
                t(L10nKey::EditorXmlShowSource).into(),
            ),
            StructuredMode::Source => (
                t(L10nKey::EditorXmlStructure).into(),
                t(L10nKey::EditorXmlShowStructure).into(),
            ),
        }
    }

    fn node_count(&self, projection: &Self::Projection) -> usize {
        projection.nodes().len()
    }

    fn display_node_count(&self, projection: &Self::Projection) -> Option<usize> {
        let _ = projection;
        None
    }

    fn count_notice(&self, projection: &Self::Projection) -> Option<SharedString> {
        Some(
            t_fmt(
                L10nKey::EditorXmlFacts,
                &[
                    ("elements", &projection.element_count().to_string()),
                    ("attributes", &projection.attribute_count().to_string()),
                ],
            )
            .into(),
        )
    }

    /// 带解析错误的投影不能用于结构编辑：树仍显示 fatal，编辑命令全部拒绝。
    fn projection_editable(&self, projection: &Self::Projection) -> bool {
        projection.error().is_none()
    }

    /// 预算失败、DTD 拒绝或整份 XML 无法解析时的可见说明。
    fn fatal_error(
        &self,
        failure: Option<&Self::Failure>,
        projection: Option<&Self::Projection>,
    ) -> Option<(SharedString, SharedString)> {
        if let Some(failure) = failure {
            let message = match failure {
                ProjectionFailure::Cancelled => return None,
                ProjectionFailure::InputTooLarge => t_fmt(
                    L10nKey::EditorXmlInputLimit,
                    &[("mib", &(XML_INPUT_BYTES / (1024 * 1024)).to_string())],
                )
                .into(),
                ProjectionFailure::TooManyNodes => t_fmt(
                    L10nKey::EditorXmlNodeLimit,
                    &[("count", &XML_NODES.to_string())],
                )
                .into(),
                ProjectionFailure::PathsTooLarge => t_fmt(
                    L10nKey::EditorXmlPathLimit,
                    &[("mib", &(XML_PATH_BYTES / (1024 * 1024)).to_string())],
                )
                .into(),
                ProjectionFailure::DtdDenied { line, column } => t_fmt(
                    L10nKey::EditorXmlDtdDeniedDetail,
                    &[("line", &line.to_string()), ("column", &column.to_string())],
                )
                .into(),
            };
            let title = match failure {
                ProjectionFailure::DtdDenied { .. } => t(L10nKey::EditorXmlDtdDenied),
                _ => t(L10nKey::EditorXmlStructureUnavailable),
            };
            return Some((title.into(), message));
        }
        let projection = projection?;
        let error = projection.error()?;
        Some((
            t(L10nKey::EditorXmlInvalid).into(),
            t_fmt(
                L10nKey::EditorXmlInvalidDetail,
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

    /// 完整值面板只对属性／文本／CDATA／注释／PI／声明成立；元素 reveal 到
    /// 源码但不提供完整值。
    fn node_has_full_value(&self, projection: &Self::Projection, index: usize) -> bool {
        projection
            .nodes()
            .get(index)
            .is_some_and(|node| node.has_full_value())
    }

    fn node_row(
        &self,
        projection: &Self::Projection,
        index: usize,
        theme: &Theme,
    ) -> Option<NodeRow> {
        let node = projection.nodes().get(index)?;
        let color = match node.kind {
            XmlNodeKind::Attribute | XmlNodeKind::Text | XmlNodeKind::Cdata => theme.green,
            XmlNodeKind::Element => theme.foreground,
            XmlNodeKind::Comment
            | XmlNodeKind::ProcessingInstruction
            | XmlNodeKind::Declaration => theme.muted_foreground,
            XmlNodeKind::Document => theme.muted_foreground,
        };
        let badge = match node.kind {
            XmlNodeKind::Element => node
                .namespace
                .as_deref()
                .map(|uri| SharedString::from(tree::truncated_chars(uri, 48))),
            XmlNodeKind::Attribute if node.is_namespace => Some(SharedString::from("xmlns")),
            XmlNodeKind::Attribute => node
                .namespace
                .as_deref()
                .map(|uri| SharedString::from(tree::truncated_chars(uri, 48))),
            XmlNodeKind::Cdata => Some(SharedString::from("CDATA")),
            XmlNodeKind::Comment => Some(SharedString::from("comment")),
            XmlNodeKind::ProcessingInstruction => Some(SharedString::from("PI")),
            XmlNodeKind::Declaration => Some(SharedString::from("declaration")),
            XmlNodeKind::Document => None,
            XmlNodeKind::Text => None,
        };
        Some(NodeRow {
            key: node.key.clone().into(),
            value: node.value.clone().into(),
            color,
            badge,
            record_line: None,
            error: None,
            aria_label: node.path.clone().into(),
        })
    }

    /// 打开时展开根与它的第一个元素子节点，复现上游的浅层可读树。
    fn default_expansion(&self, projection: &Self::Projection, expanded: &mut [bool]) {
        let Some(root) = projection.root() else {
            return;
        };
        if root >= expanded.len() {
            return;
        }
        expanded[root] = true;
        for &child in projection.children(root) {
            if projection.nodes()[child].kind == XmlNodeKind::Element {
                if child < expanded.len() {
                    expanded[child] = true;
                }
                break;
            }
        }
    }

    fn edit_buttons(
        &self,
        projection: &Self::Projection,
        selected: Option<usize>,
    ) -> Vec<EditButton<Self::EditRequest>> {
        let mut caps_value = false;
        let mut caps_rename = false;
        let mut caps_add_attribute = false;
        let mut caps_add_child = false;
        let mut caps_add_text = false;
        let mut caps_remove = false;
        if let Some(node) = selected.and_then(|index| projection.nodes().get(index)) {
            caps_value = node.kind.is_value();
            caps_rename = matches!(node.kind, XmlNodeKind::Element | XmlNodeKind::Attribute);
            caps_add_attribute = node.kind == XmlNodeKind::Element;
            caps_add_child = node.kind == XmlNodeKind::Element;
            caps_add_text = node.kind == XmlNodeKind::Element;
            caps_remove = match node.kind {
                XmlNodeKind::Element => !node.is_element_root(),
                XmlNodeKind::Attribute
                | XmlNodeKind::Text
                | XmlNodeKind::Cdata
                | XmlNodeKind::Comment => true,
                _ => false,
            };
        }
        vec![
            EditButton {
                id: "edit-value",
                label: t(L10nKey::EditorXmlEditValue).into(),
                enabled: caps_value,
                flow: EditFlow::Panel(XmlEditRequest::Value),
            },
            EditButton {
                id: "rename",
                label: t(L10nKey::EditorXmlRename).into(),
                enabled: caps_rename,
                flow: EditFlow::Panel(XmlEditRequest::Rename),
            },
            EditButton {
                id: "add-attribute",
                label: t(L10nKey::EditorXmlAddAttribute).into(),
                enabled: caps_add_attribute,
                flow: EditFlow::Panel(XmlEditRequest::AddAttribute),
            },
            EditButton {
                id: "add-child",
                label: t(L10nKey::EditorXmlAddChild).into(),
                enabled: caps_add_child,
                flow: EditFlow::Panel(XmlEditRequest::AddChild),
            },
            EditButton {
                id: "add-text",
                label: t(L10nKey::EditorXmlAddText).into(),
                enabled: caps_add_text,
                flow: EditFlow::Panel(XmlEditRequest::AddText),
            },
            EditButton {
                id: "remove",
                label: t(L10nKey::EditorXmlRemove).into(),
                enabled: caps_remove,
                flow: EditFlow::Immediate(XmlEditRequest::Remove),
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
            // 值编辑：属性单行、文本／CDATA／注释多行；命名空间节点给出提示。
            XmlEditRequest::Value => {
                let node = selected.and_then(|index| projection.nodes().get(index))?;
                if !node.kind.is_value() {
                    return None;
                }
                let raw = node.value_range.clone().and_then(|range| text.get(range))?;
                let (title_key, label, multiline) = match node.kind {
                    XmlNodeKind::Attribute => (
                        L10nKey::EditorXmlEditAttributeValueTitle,
                        L10nKey::EditorXmlAttributeValue,
                        false,
                    ),
                    XmlNodeKind::Text => (
                        L10nKey::EditorXmlEditTextTitle,
                        L10nKey::EditorXmlTextValue,
                        true,
                    ),
                    XmlNodeKind::Cdata => (
                        L10nKey::EditorXmlEditCdataTitle,
                        L10nKey::EditorXmlCdataValue,
                        true,
                    ),
                    XmlNodeKind::Comment => (
                        L10nKey::EditorXmlEditCommentTitle,
                        L10nKey::EditorXmlCommentValue,
                        true,
                    ),
                    _ => return None,
                };
                let notice = node
                    .is_namespace
                    .then(|| SharedString::from(t(L10nKey::EditorXmlNamespaceNotice)));
                Some(EditPlan::Panel(EditPanelDesc {
                    title: t_fmt(title_key, &[("path", &node.path)]).into(),
                    key: None,
                    value: EditField::new(t(label), t(L10nKey::EditorXmlValuePlaceholder), raw),
                    target: NodeTarget::of(node),
                    notice,
                    multiline,
                }))
            }
            XmlEditRequest::Rename => {
                let node = selected.and_then(|index| projection.nodes().get(index))?;
                if !matches!(node.kind, XmlNodeKind::Element | XmlNodeKind::Attribute) {
                    return None;
                }
                let notice = node
                    .is_namespace
                    .then(|| SharedString::from(t(L10nKey::EditorXmlNamespaceNotice)));
                Some(EditPlan::Panel(EditPanelDesc {
                    title: t_fmt(L10nKey::EditorXmlRenameTitle, &[("path", &node.path)]).into(),
                    key: None,
                    value: EditField::new(
                        t(L10nKey::EditorXmlName),
                        t(L10nKey::EditorXmlNamePlaceholder),
                        node.key.clone(),
                    ),
                    target: NodeTarget::of(node),
                    notice,
                    multiline: false,
                }))
            }
            XmlEditRequest::AddAttribute => {
                let node = selected.and_then(|index| projection.nodes().get(index))?;
                if node.kind != XmlNodeKind::Element {
                    return None;
                }
                Some(EditPlan::Panel(EditPanelDesc {
                    title: t_fmt(L10nKey::EditorXmlAddAttributeTitle, &[("path", &node.path)])
                        .into(),
                    key: Some(EditField::new(
                        t(L10nKey::EditorXmlName),
                        t(L10nKey::EditorXmlNamePlaceholder),
                        "",
                    )),
                    value: EditField::new(
                        t(L10nKey::EditorXmlAttributeValue),
                        t(L10nKey::EditorXmlValuePlaceholder),
                        "",
                    ),
                    target: NodeTarget::of(node),
                    notice: None,
                    multiline: false,
                }))
            }
            XmlEditRequest::AddChild => {
                let node = selected.and_then(|index| projection.nodes().get(index))?;
                if node.kind != XmlNodeKind::Element {
                    return None;
                }
                Some(EditPlan::Panel(EditPanelDesc {
                    title: t_fmt(L10nKey::EditorXmlAddChildTitle, &[("path", &node.path)]).into(),
                    key: None,
                    value: EditField::new(
                        t(L10nKey::EditorXmlName),
                        t(L10nKey::EditorXmlNamePlaceholder),
                        "",
                    ),
                    target: NodeTarget::of(node),
                    notice: None,
                    multiline: false,
                }))
            }
            XmlEditRequest::AddText => {
                let node = selected.and_then(|index| projection.nodes().get(index))?;
                if node.kind != XmlNodeKind::Element {
                    return None;
                }
                Some(EditPlan::Panel(EditPanelDesc {
                    title: t_fmt(L10nKey::EditorXmlAddTextTitle, &[("path", &node.path)]).into(),
                    key: None,
                    value: EditField::new(
                        t(L10nKey::EditorXmlTextValue),
                        t(L10nKey::EditorXmlTextPlaceholder),
                        "",
                    ),
                    target: NodeTarget::of(node),
                    notice: None,
                    multiline: true,
                }))
            }
            XmlEditRequest::Remove => {
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
            XmlEditRequest::Value => Mutation::SetValue {
                target: target.clone(),
                value,
            },
            XmlEditRequest::Rename => Mutation::Rename {
                target: target.clone(),
                name: value,
            },
            XmlEditRequest::AddAttribute => Mutation::AddAttribute {
                element: target.clone(),
                name: key.unwrap_or_default(),
                value,
            },
            XmlEditRequest::AddChild => Mutation::AddChild {
                parent: target.clone(),
                name: value,
            },
            XmlEditRequest::AddText => Mutation::AddText {
                parent: target.clone(),
                text: value,
            },
            XmlEditRequest::Remove => Mutation::Remove {
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
        mutation::plan(text, projection, mutation)
    }

    fn mutation_message(
        &self,
        error: StructuredMutationError<Self::MutationError>,
    ) -> SharedString {
        match error {
            StructuredMutationError::Stale => t(L10nKey::EditorXmlEditStale).into(),
            StructuredMutationError::Plan(MutationError::Stale) => {
                t(L10nKey::EditorXmlEditStale).into()
            }
            StructuredMutationError::Plan(MutationError::Unsupported) => {
                t(L10nKey::EditorXmlEditUnsupported).into()
            }
            StructuredMutationError::Plan(MutationError::InvalidName(message)) => {
                t_fmt(L10nKey::EditorXmlEditInvalidName, &[("message", &message)]).into()
            }
            StructuredMutationError::Plan(MutationError::InvalidValue(message)) => {
                t_fmt(L10nKey::EditorXmlEditInvalidValue, &[("message", &message)]).into()
            }
            StructuredMutationError::Plan(MutationError::InvalidCdata) => {
                t(L10nKey::EditorXmlEditInvalidCdata).into()
            }
            StructuredMutationError::Plan(MutationError::InvalidComment) => {
                t(L10nKey::EditorXmlEditInvalidComment).into()
            }
            StructuredMutationError::Plan(MutationError::ResultInvalid {
                line,
                column,
                message,
            }) => t_fmt(
                L10nKey::EditorXmlEditResultInvalid,
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
        t(L10nKey::EditorXmlParsing).into()
    }

    fn unavailable(&self) -> (SharedString, SharedString) {
        (
            t(L10nKey::EditorXmlStructureUnavailable).into(),
            t(L10nKey::EditorXmlSourceStaysEditable).into(),
        )
    }

    fn full_value_label(&self) -> SharedString {
        t(L10nKey::EditorXmlFullValue).into()
    }

    fn copy_value_label(&self) -> SharedString {
        t(L10nKey::EditorXmlCopyValue).into()
    }

    fn reveal_label(&self) -> SharedString {
        t(L10nKey::EditorXmlReveal).into()
    }
}

/// 一份 XML 文件的内容：共享 Document 与按最新 revision 安装的投影。
pub type XmlDocument = StructuredDocument<XmlShell>;

/// 一个 presentation 里的 XML 呈现：模式、源码选择、结构展开与滚动。
pub type XmlView = StructuredView<XmlShell>;

/// XML 呈现模式：结构树或源码。
pub type XmlMode = StructuredMode;

/// 结构预览持有键盘时成立的上下文；源码面不声明它，文字编辑键不会被截走。
pub const KEY_CONTEXT: &str = <XmlShell as StructuredViewFormat>::KEY_CONTEXT;

/// 右侧菜单的 XML 项：结构／源码切换；其余编辑命令仍由普通文本菜单提供。
pub fn menu_entries(
    menu: gpui_kit::component::menu::PopupMenu,
) -> gpui_kit::component::menu::PopupMenu {
    super::structured_view::menu_entries(menu, L10nKey::EditorXmlToggleStructure)
}

impl StructuredView<XmlShell> {
    /// 元素计数（状态栏与工具栏计数）。
    pub fn element_count(&self, cx: &gpui_kit::App) -> Option<usize> {
        self.document()
            .read(cx)
            .projection()
            .map(XmlProjection::element_count)
    }

    /// 属性计数（状态栏与工具栏计数）。
    pub fn attribute_count(&self, cx: &gpui_kit::App) -> Option<usize> {
        self.document()
            .read(cx)
            .projection()
            .map(XmlProjection::attribute_count)
    }
}

/// 解析的确定性单元测试门闩，不编译进产品或独立 native 消费者。
#[cfg(any(test, feature = "test-support"))]
pub use super::structured_document::test_support;
