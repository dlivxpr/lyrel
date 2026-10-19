# 分诊标签

工程技能使用五种规范分诊角色。下表将角色映射到本仓库工单使用的状态字符串。

| 规范角色 | 本仓库标签 | 含义 |
|---|---|---|
| `needs-triage` | `needs-triage` | 等待维护者评估 |
| `needs-info` | `needs-info` | 等待报告者补充信息 |
| `ready-for-agent` | `ready-for-agent` | 规格完整，可交由无人值守 agent 实施 |
| `ready-for-human` | `ready-for-human` | 需要人工实施 |
| `wontfix` | `wontfix` | 不予处理 |

技能提到某个分诊角色时，使用表中对应的标签字符串。本地 Markdown 工单通过顶部附近的 `Status:` 行记录该值。

以后若需更换标签名称，修改“本仓库标签”列。
