# 工单跟踪：GitHub

本仓库的 issue 与规格存放在 GitHub Issues。所有操作使用 `gh` CLI；仓库从 `git remote -v` 推断，在克隆目录内运行 `gh` 会自动识别。

## 约定

- **创建 issue**：`gh issue create --title "..." --body "..."`；多行正文用 heredoc。
- **读取 issue**：`gh issue view <number> --json number,title,body,labels,comments`。
- **列出 issue**：`gh issue list --state open --json number,title,body,labels,comments --jq '[.[] | {number, title, body, labels: [.labels[].name], comments: [.comments[].body]}]'`，配合相应的 `--label`、`--state` 过滤。
- **将 issue 设为父 issue 的子 issue**：`gh issue create --parent <parent> ...`，或事后 `gh issue edit <parent> --add-sub-issue <child>`（需要 `gh` 2.94+）。旧版 `gh`：`gh api --method POST repos/<owner>/<repo>/issues/<parent>/sub_issues -F sub_issue_id=<child-db-id>`（child 的数据库 id）。不支持子 issue 时，在子 issue 正文顶部写 `Part of #<parent>`。
- **评论**：`gh issue comment <number> --body "..."`。
- **添加／移除标签**：`gh issue edit <number> --add-label "..."` / `--remove-label "..."`。
- **关闭**：`gh issue close <number> --comment "..."`。

## PR 作为请求入口

**PR 作为请求入口：否。** _（若本仓库把外部 PR 当特性请求处理，改为 `yes`；`/triage` 读取该标记。）_

设为 `yes` 时，PR 走与 issue 相同的标签与状态，命令用 `gh pr` 对应版本：

- **读取 PR**：`gh pr view <number> --comments`；diff 用 `gh pr diff <number>`。
- **列出待分诊的外部 PR**：`gh api --paginate 'repos/{owner}/{repo}/pulls?state=open' --jq '.[] | select(.author_association | IN("OWNER","MEMBER","COLLABORATOR") | not) | {number, title, author: .user.login, author_association, labels: [.labels[].name]}'`。
- **评论／加标签／关闭**：`gh pr comment`、`gh pr edit --add-label`/`--remove-label`、`gh pr close`。

GitHub 的 issue 与 PR 共用编号空间，裸 `#42` 可能是两者之一：先用 `gh pr view 42` 判断，再回退到 `gh issue view 42`。

## 技能要求“发布到 issue tracker”时

创建一个 GitHub issue。

## 技能要求“获取相关工单”时

按上文“读取 issue”读取。

## Wayfinding 操作

供 `/wayfinder` 使用。**map** 是单个 issue，**child** issue 是它的票。

- **Map**：带 `wayfinder:map` 标签的单个 issue，正文保存 Notes / Decisions-so-far / Fog。`gh issue create --label wayfinder:map`。
- **Child 票**：通过 GitHub sub-issue 关联到 map（见上文“将 issue 设为父 issue 的子 issue”）。不支持 sub-issue 时，把 child 加入 map 正文的任务列表，并在 child 正文顶部写 `Part of #<map>`。标签：`wayfinder:<type>`（`research`/`prototype`/`grilling`/`task`）。认领后指派给负责的开发。
- **阻塞**：使用 GitHub 原生 issue 依赖（界面上可见的标准表示）。加边：`gh api --method POST repos/<owner>/<repo>/issues/<child>/dependencies/blocked_by -F issue_id=<blocker-db-id>`，其中 `<blocker-db-id>` 是阻塞者的数字**数据库 id**（`gh api repos/<owner>/<repo>/issues/<n> --jq .id`，不是 `#number`，也不是 `node_id`）。GitHub 以 `issue_dependencies_summary.blocked_by` 报告（只计未关闭的阻塞者，是实时闸门）。不支持依赖时，退化为 child 正文顶部的 `Blocked by: #<n>, #<n>` 行。所有阻塞者关闭后该票解除阻塞。
- **前沿查询**：列出 map 的未关闭 child（`gh issue list --state open`，限定在 map 的 sub-issue／任务列表内），剔除有未关闭阻塞者（`issue_dependencies_summary.blocked_by > 0`，或 `Blocked by` 行中仍有未关闭 issue）或已有指派者的；按 map 顺序取第一个。
- **认领**：`gh issue edit <n> --add-assignee @me`，这是会话的第一次写操作。
- **解决**：`gh issue comment <n> --body "<答案>"` → `gh issue close <n>` → 把上下文指针（gist + 链接）追加到 map 的 Decisions-so-far。

## 本仓库约定

- 仅在用户明确要求时创建 issue、规格等工单；不自动生成文档目录或工单树。
- 用户提供 issue 编号、规格路径或外部链接时，先读取其实际内容；不猜测编号。
- 完成任务时记录实际结果与证据边界；部分成功、前提缺失和未验证项不得写成完成。
