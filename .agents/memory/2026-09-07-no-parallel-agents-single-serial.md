# 2026-09-07 · 禁多代理并发——一律单代理串行

## 决策（用户拍板）

6 维并行审查 Workflow（parallel 6 agents）直接打爆 API 导致中断，用户明确指示：**先不使用 workflow，改成单个子代理**。

## 规则（本机环境约束，长期有效）

- 审查、修复、文档类任务：**单个子代理串行**执行，或主代理自己做；禁止 parallel() 并发扇出多个 agent。
- 待本机 API 配额/稳定性确认可承受后再评估小规模（≤2）并发；在此之前默认串行。

## 关联

- [[2026-09-07-p2p-direction-a-no-server-auto-share-origin]]
- [[2026-09-07-receipt-ui-and-spectator-design]]
