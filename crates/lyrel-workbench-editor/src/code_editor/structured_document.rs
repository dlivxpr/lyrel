//! 共享结构化文档编排：一份共享逐行保真源码，加上按 revision 派生的无损
//! 结构投影。
//!
//! JSON／JSONC／JSONL、TOML（以及后续的 YAML／XML）的文档侧只有三件事是
//! 真共享的：订阅共享 [`Document`]、把解析放到 UI 线程外单飞执行、只安装与
//! 当前 revision／generation 匹配的结果。格式自己的解析与投影模型由
//! [`StructuredDocumentFormat`] 提供；这里不持有任何格式的节点语义，也不
//! 复制第二份可变内容。
//!
//! 解析输入有界、每份文档最多一个 worker 与一次在途解析；解析失败或超预算
//! 时源码仍然可编辑。结构视图的展开、选择、源码定位和完整值属于每个
//! presentation（见 `structured_view`）；文件自己只持有 Document、格式壳与
//! 最新的投影。

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{Context, Entity, Subscription, Task};
use lyrel_document::state::CancelToken;
use lyrel_document::tree::Treelike;
use lyrel_document::{ProjectionResult, ProjectionState};
use lyrel_markdown_editor::{Document, DocumentEvent};

/// 合并连续输入；每份文档最多一个 worker 与一次在途解析。
const PARSE_DEBOUNCE: Duration = Duration::from_millis(120);

/// 结构化文档格式壳：只提供格式自己的解析与投影/失败类型。
///
/// `lyrel-document` 里的 per-format projection/mutation 模型保持独立无损；
/// 共享代码只依赖 [`Treelike`] 遍历投影，不引入跨格式的最小公分母节点。
pub trait StructuredDocumentFormat: Clone + Send + 'static {
    /// 按 revision 安装的保真投影。
    type Projection: Treelike + Send + 'static;
    /// 解析失败或超出预算的失败标记。
    type Failure: Clone + Send + 'static;

    /// 在后台线程解析给定文本；`cancel` 置位时这次结果必须作废，不得安装。
    fn parse(&self, text: &str, cancel: &CancelToken) -> Result<Self::Projection, Self::Failure>;
}

/// 一份结构化文件的内容：共享 Document、格式壳与按最新 revision 安装的投影。
pub struct StructuredDocument<F: StructuredDocumentFormat> {
    document: Entity<Document>,
    format: F,
    state: ProjectionState<F::Projection, F::Failure>,
    _worker: Option<Task<()>>,
    _document: Subscription,
}

impl<F: StructuredDocumentFormat> Drop for StructuredDocument<F> {
    fn drop(&mut self) {
        self.state.cancel();
    }
}

impl<F: StructuredDocumentFormat> StructuredDocument<F> {
    pub fn new(document: Entity<Document>, format: F, cx: &mut Context<Self>) -> Self {
        let watch = cx.subscribe(&document, |this, _, _: &DocumentEvent, cx| {
            this.invalidate(cx)
        });
        let mut this = Self {
            document,
            format,
            state: ProjectionState::new(),
            _worker: None,
            _document: watch,
        };
        this.invalidate(cx);
        this
    }

    /// 共享的逐行保真源码；保存、撤销、reload 与关闭都走既有工作台生命周期。
    pub fn document(&self) -> &Entity<Document> {
        &self.document
    }

    /// 这份文档的格式壳（JSON 家族带具体 `JsonFormat`，TOML 等为单位壳）。
    pub fn format(&self) -> &F {
        &self.format
    }

    pub fn projection(&self) -> Option<&F::Projection> {
        self.state.projection()
    }

    pub fn shared_projection(&self) -> Option<Arc<F::Projection>> {
        self.state.shared_projection()
    }

    pub fn failure(&self) -> Option<F::Failure> {
        self.state.failure()
    }

    pub fn is_parsing(&self) -> bool {
        self.state.is_parsing()
    }

    pub fn installed_revision(&self) -> Option<u64> {
        self.state.installed_revision()
    }

    fn invalidate(&mut self, cx: &mut Context<Self>) {
        if self.state.invalidate() {
            self.start_worker(cx);
        }
        cx.notify();
    }

    /// 单飞 worker：debounce 后取最新待解析快照，只安装 generation 与
    /// revision 都仍匹配的结果；debounce 合并掉全部编辑时收工。
    fn start_worker(&mut self, cx: &mut Context<Self>) {
        self.state.begin_worker();
        self._worker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PARSE_DEBOUNCE).await;
                let job = this
                    .update(cx, |this, cx| {
                        let Some(job) = this.state.take_job() else {
                            cx.notify();
                            return None;
                        };
                        let document = this.document.read(cx);
                        Some((
                            document.snapshot(),
                            document.revision(),
                            this.format.clone(),
                            job,
                        ))
                    })
                    .ok()
                    .flatten();
                let Some((snapshot, revision, format, job)) = job else {
                    break;
                };
                let generation = job.generation();
                let cancel = job.cancel().clone();
                #[cfg(any(test, feature = "test-support"))]
                let gate = test_support::take_next_parse();
                let result = lyrel_ui_base::host_ops::off_thread(move || {
                    if cancel.is_cancelled() {
                        return None;
                    }
                    #[cfg(any(test, feature = "test-support"))]
                    if let Some(gate) = gate {
                        gate.block();
                    }
                    Some(format.parse(snapshot.text(), &cancel))
                })
                .await
                .flatten();
                let result = match result {
                    Some(Ok(projection)) => ProjectionResult::Ready(projection),
                    Some(Err(failure)) => ProjectionResult::Failed(failure),
                    None => ProjectionResult::Cancelled,
                };
                let keep_going = this
                    .update(cx, |this, cx| {
                        let current = this.document.read(cx).revision();
                        let keep_going = this.state.install(generation, revision, current, result);
                        cx.notify();
                        keep_going
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        }));
    }
}

/// 解析的确定性单元测试门闩，不编译进产品或独立 native 消费者。
///
/// 一个门闩只暂停下一次 worker 解析：发送端释放它，`entered` 报告 worker
/// 已到达。JSON／TOML（以及后续的 YAML／XML）共用同一线程本地槽，因为同一
/// 测试线程上任意时刻只有一个解析会被暂停。
#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::{cell::RefCell, sync::mpsc, time::Duration};

    pub struct ParseGate {
        receiver: mpsc::Receiver<()>,
        entered: Arc<AtomicBool>,
    }

    thread_local! {
        static GATE: RefCell<Option<ParseGate>> = const { RefCell::new(None) };
    }

    /// Pauses the next worker parse before it reads the projection. The
    /// returned sender releases it; the flag reports that the worker arrived.
    pub fn pause_next_parse() -> (mpsc::Sender<()>, Arc<AtomicBool>) {
        let (tx, rx) = mpsc::channel();
        let entered = Arc::new(AtomicBool::new(false));
        GATE.with_borrow_mut(|gate| {
            *gate = Some(ParseGate {
                receiver: rx,
                entered: entered.clone(),
            })
        });
        (tx, entered)
    }

    pub fn take_next_parse() -> Option<ParseGate> {
        GATE.with_borrow_mut(Option::take)
    }

    impl ParseGate {
        pub fn block(self) {
            self.entered.store(true, Ordering::SeqCst);
            let _ = self.receiver.recv_timeout(Duration::from_secs(20));
        }
    }
}
