# 分诊标签

工程技能使用五种规范分诊角色。下表把角色映射到本仓库 GitHub issue 实际使用的标签。

| 规范角色 | 本仓库标签 | 含义 |
|---|---|---|
| `needs-triage` | `needs-triage` | 等待维护者评估 |
| `needs-info` | `needs-info` | 等待报告者补充信息 |
| `ready-for-agent` | `ready-for-agent` | 规格完整，可交由无人值守 agent 实施 |
| `ready-for-human` | `ready-for-human` | 需要人工实施 |
| `wontfix` | `wontfix` | 不予处理 |

技能提到某个分诊角色时，使用表中对应的标签字符串；在 GitHub 上通过 `gh issue edit <number> --add-label "<标签>"` 应用。

以后若需更换标签名称，修改“本仓库标签”列，并同步 GitHub 仓库标签。
