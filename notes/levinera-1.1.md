# Levinera `new1.1` 改动记录

> 本文件用于存档：GitHub 上的 `new1.1` 分支及其提交已被删除，
> 其代码内容已作为一次干净提交并入 `main`。此处记录原作者与改动内容。

## 背景

- 分支：`new1.1`（远程 `origin/new1.1`，已删除）
- 作者：Levinera `<2418716509@qq.com>`，提交时间 2026-09-12
- 涉及提交（4 个，均不在 `main` 上）：

  | 提交 | 说明 |
  | --- | --- |
  | `50683d6` | `1.1版` —— 独立根提交，把整个项目重新提交了一遍（不是从 main 分出来的） |
  | `33d8662` | `feat: update scratch card game` —— 玩法重做的主体改动 |
  | `9b0acd4` | `merge remote new1.1 history` —— 空合并，把两条无共同祖先的历史接起来 |
  | `7a2e24d` | `chore: preserve remote new1.1 test file` —— 补回合并时丢失的 `laoeeee_test.txt` |

- 备份：本地 tag `backup/new1.1-levinera`，以及 `levinera-new1.1-backup.bundle`。

## 改动内容（相对 main 的 v1.0.0）

玩法从「3×3 九宫格凑三连」重做成「**一页 5 格、每格独立开奖、显示金额、可翻页**」。

| 方面 | 旧（main / v1.0.0） | 新（new1.1 / 1.1） |
| --- | --- | --- |
| 玩法 | 3×3 九宫格，刮出三连（横/竖/斜）中奖 | 一页最多 5 格，每格**独立**开奖 |
| 奖励 | emoji 符号（💎💰🎁🍀） | **金额数字**（`$20` / `$0`） |
| 发牌 | 整卡先定"哪条线三连" | 每格独立按概率抽奖 |
| 流程 | 刮开→结算→再来一张 | **设置次数**→一页 5 格→**翻页**继续 |

### 后端

- `backend/src/models.rs`
  - `GRID_SIZE = 9` → `MAX_PAGE_CELLS = 5`
  - `Cell { id, symbol, revealed }` → `Cell { id, amount: u32, label: String, revealed }`
  - `Game` 去掉 `reward / tier_name / win_symbol`（不再"整卡一个奖"）
  - 新增 `NewCardRequest { count: Option<u32> }`（前端可指定这一页几格）
  - `RevealResponse`：`symbol` → `amount / label / win`
  - `FinishResponse`：去掉 `tier / symbol`
- `backend/src/game.rs`
  - **删除**整套三连逻辑：`LINES`、`winning_symbol()`、`random_filler()`、`build_grid()`
  - `Game::create(config)` → `Game::create(config, requested)`：每格独立抽奖，格数夹到 `1..=page_size`
  - `finish()` 改为只累加**已刮开**格子的金额
- `backend/src/config.rs`
  - 新增字段 `page_size`（默认 5）、`max_count`（默认 100）、`lose_label`（默认 `$0`）
  - `filler_symbols` 变为遗留字段（`#[serde(default)]`，已不生效）
  - 默认奖级改为美元：特等奖 $1000 / 一等奖 $100 / 二等奖 $20 / 三等奖 $5
  - `reveal_threshold` 默认 `0.4 → 0.7`
- `backend/src/api.rs`
  - `POST /api/game/new` 支持请求体 `{"count": n}`，缺失/解析失败按满页处理
  - `GET /api/config` 增加返回 `page_size / max_count / lose_label`
- `config/game.json` 同步以上新字段与金额

### 前端

- `web/index.html`：单屏改为**两屏** —— `#setup`（设置次数：快捷 chips + 加减步进器）与
  `#game`（计数器 未刮/已刮/第几页、动态生成的 `.slot` 格子、本页/今日累计、翻页与重设按钮）
- `web/assets/app.js`：近乎重写，适配新的 DOM 结构与"一页多格、翻页、逐格开奖"流程
- `web/assets/style.css`：新增 `.chips / .counter / .slot / .slot.win`（中奖格黄色渐变）等样式

### 脚本与文档

- `scripts/check-pi.sh`：新增"默认发 5 格""`{"count":3}` 发 3 格"校验；reveal 断言由 `symbol` 改为 `label`+`amount`
- `scripts/e2e-browser.mjs`：端到端流程重写（设置 10 次 → 刮满第 1 页 → 翻页 → 刮完 → 触摸路径）
- `README.md`：新增「玩法」章节，API 与配置表更新
- `.gitignore`：新增 `/exit`

## 处理方式

原 `new1.1` 是一条**独立起源**的历史（与新仓库无共同祖先），由"拷贝文件 + 重新 init"产生，
并靠一次空合并勉强接上，导致 `main` 的文件一度被吞。为保持历史干净：

1. 备份 `origin/new1.1`（tag + bundle）；
2. 把 1.1 的**代码内容**作为一次新提交并入 `main`（历史中不再保留其原始提交）；
3. 删除本地与远程的 `new1.1` 分支。

> 注：`daily_limit` 仍只在浏览器端限制用户可填次数，服务端不强制，与旧版一致。
