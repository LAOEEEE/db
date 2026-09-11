# 幸运刮刮乐 v1.0.0 发行说明

> 一个运行在树莓派上的轻量级网页刮刮乐小游戏。
> 单二进制、无数据库、无前端构建步骤，解包即用。

---

## 版本信息

| 项目 | 内容 |
| --- | --- |
| 版本号 | **v1.0.0** |
| 发布日期 | 2026-09-11 |
| 目标平台 | 树莓派 3B / 树莓派 OS（ARM64 Linux，aarch64） |
| 运行环境 | 无需安装任何运行时（二进制静态链接），推荐 512MB+ 内存 |
| 语言 / 框架 | Rust 2024 + Axum 0.8 + Tokio；前端原生 HTML/CSS/JS |
| 发行包 | `scratch-card v1.0.0.rar`（约 580 KB） |
| 授权 / 性质 | 演示用小游戏，**非彩票、非博彩系统** |

---

## 发行包内容

发行包由 `scripts/build.sh` 自动打包生成，解压后目录结构如下：

```text
scratch-card v1.0.0/
├── scratch-card-server          # 可执行文件（静态链接，约 1.5 MB）
├── web/                         # 前端静态文件
│   ├── index.html
│   └── assets/
│       ├── app.js               # 刮奖交互逻辑（Canvas 擦除）
│       └── style.css            # 样式（移动端适配）
├── config/
│   └── game.json                # 游戏规则配置（奖级/概率/阈值）
└── scratch-card.service         # systemd 服务单元（开机自启）
```

**注意**：`scratch-card-server` 是为 ARM64 Linux 交叉编译的 ELF 可执行文件，
**不能在 Windows / x86-64 机器上直接运行**，只能在树莓派或其它 ARM64 Linux 设备上运行。

---

## 主要功能

### 服务端（Rust + Axum）

- **单二进制同时提供 API 与静态文件**：一个进程既处理后端接口，也返回前端页面，无需额外的 Nginx。
- **中奖结果完全由后端决定**：发牌瞬间就定好中奖与否，前端拿不到真实图案，无法作弊。
- **公平的发牌算法**：
  - 按累积概率表抽取奖级（默认：一等奖 0.1%、二等奖 1%、三等奖 5%、安慰奖 20%，其余未中奖）。
  - 中奖卡**保证有且仅有一条三连**（横 / 竖 / 斜 共 8 种线），其余格子为随机填充符号。
  - 未中奖卡**保证一条三连都没有**。
- **独一无二的卡片 ID**：由操作系统真随机数生成 32 位十六进制字符串，碰撞概率可忽略。
- **内存存储 + 自动清理**：卡片存在内存 `HashMap` 中，上限 2000 张；超限时先清理已结算的卡，再按创建时间淘汰最旧的一半。
- **全局限流**：默认每个 IP 每 10 秒最多 120 次请求，超限返回 `429`，防止被刷爆。
- **请求体大小限制**：单次请求体最大 4 KB。
- **配置校验**：启动时校验配置合法性（概率之和 ≤ 1、填充符号 ≥ 3 个、阈值在 5%~100% 之间等），坏配置直接拒绝启动。
- **完善的单元测试**：发牌规则、概率分布、边界条件均有测试覆盖（10 万次抽样验证概率误差 < 1%）。

### 前端（原生 HTML/CSS/JS）

- **Canvas 刮奖**：用鼠标或手指（Pointer 事件，同时支持桌面和触摸屏）在涂层上擦除，还原真实刮卡手感。
- **本地判定刮开面积**：每 48×48 采样一次 Canvas，刮开面积达到阈值即自动结算，避免鼠标移动产生大量请求。
- **自动结算条件**：刮开面积达到 `reveal_threshold`（默认 90%）**或** 9 个图案全部出现，任一满足即结算。
- **移动端适配**：viewport 适配、禁止缩放、涂层按设备像素比（DPR）高清渲染。
- **本地统计**：用 `localStorage` 记录当日积分与剩余次数，按日期自动重置，不依赖账号系统。
- **中奖动效**：中奖时整卡高亮、中奖符号描边。

---

## API 一览

所有接口以 `/api` 为前缀，返回 JSON。

| 方法 | 路径 | 请求体 | 说明 |
| --- | --- | --- | --- |
| GET | `/api/health` | — | 健康检查，返回 `{"status":"ok"}` |
| GET | `/api/config` | — | 返回公开配置 `{daily_limit, reveal_threshold}`（**不含**奖级与概率） |
| POST | `/api/game/new` | `{}` | 新建卡片，返回 `{card_id, cells:[{id}...]}`，**不返回图案** |
| POST | `/api/game/reveal` | `{card_id, cell_id}` | 刮开一格，返回 `{cell_id, symbol}` |
| POST | `/api/game/finish` | `{card_id}` | 结算（仅限一次），返回中奖结果 |

**错误响应**统一为 `{"error": 错误码, "message": 中文说明}`：

| HTTP | error 码 | 含义 |
| --- | --- | --- |
| 400 | `invalid_request` | 请求 JSON 格式不对或参数为空 |
| 400 | `invalid_cell` | 格子编号不在 0~8 范围内 |
| 404 | `card_not_found` | 卡片不存在或已被清理 |
| 409 | `already_revealed` | 该格子已刮开 |
| 409 | `already_finished` | 该卡片已结算，不能重复领取 |
| 429 | `rate_limited` | 请求过于频繁 |
| 500 | `internal_error` | 服务器内部错误 |

---

## 配置说明（config/game.json）

游戏规则不写死在代码里，改这个文件即可调整（改完重启服务生效）。

```json
{
  "rewards": [
    { "name": "一等奖", "symbol": "💎", "value": 1000, "probability": 0.001 },
    { "name": "二等奖", "symbol": "💰", "value": 100,  "probability": 0.01 },
    { "name": "三等奖", "symbol": "🎁", "value": 20,   "probability": 0.05 },
    { "name": "安慰奖", "symbol": "🍀", "value": 5,    "probability": 0.2 }
  ],
  "filler_symbols": ["⭐", "🍒", "🔔", "🍋", "🍇", "🍉"],
  "daily_limit": 0,
  "reveal_threshold": 0.9
}
```

| 字段 | 说明 | 约束 |
| --- | --- | --- |
| `rewards` | 奖级列表（名称 / 符号 / 积分 / 概率） | 不能为空；概率 0~1，且所有概率之和 ≤ 1 |
| `filler_symbols` | 未中奖格子的填充符号 | 至少 3 个 |
| `daily_limit` | 每日可抽取次数，**0 表示不限次数** | 缺失时默认 10 |
| `reveal_threshold` | 刮开面积达到此比例自动结算 | 必须在 0.05 ~ 1.0 之间，缺失时默认 0.4 |

> **本发行版的当前取值**：`daily_limit = 0`（不限次数）、`reveal_threshold = 0.9`（刮开 90% 自动结算）。

### ⚠️ 关于"每日次数限制"的重要说明

`daily_limit` **只在浏览器端（localStorage）生效，服务端并不强制**。
也就是说：清空浏览器缓存、换个浏览器或直接调接口，都能绕过次数限制。
本项仅用于演示和小范围娱乐，**不具备防刷能力**。若需真正限制次数，需要改后端
（例如按 IP 记录当日已用次数）。

---

## 部署方式

### 一、手动运行（临时验证）

把发行包解压到树莓派任意目录，在该目录下执行：

```bash
./scratch-card-server
```

默认监听 `0.0.0.0:3000`，浏览器访问 `http://<树莓派IP>:3000` 即可。

> 注意：这样运行会随 SSH 断开而停止，且不会开机自启。

### 二、安装为 systemd 服务（推荐，长期运行）

1. 把程序、`web/`、`config/` 放到 `/opt/scratch-card/`（或自定义目录）。
2. 把 `scratch-card.service` 放到 `/etc/systemd/system/`，并**把其中的 `User=` 改成你的登录用户名**。
3. 启用并启动：

   ```bash
   sudo systemctl daemon-reload
   sudo systemctl enable scratch-card     # 开机自启
   sudo systemctl restart scratch-card    # 立即启动
   sudo systemctl status scratch-card     # 查看状态
   sudo journalctl -u scratch-card -f     # 查看实时日志
   ```

> 若树莓派没有免密 sudo，可改用**用户级服务**：文件放 `~/.config/systemd/user/`，
> 命令加 `--user`，并执行一次 `sudo loginctl enable-linger <用户名>` 以支持开机自启。
> `scratch-card.service` 内已含逐行中文注释，说明每一项怎么改。

### 三、从源码重新构建（开发）

```bash
# 本地开发（任意平台，x86-64 亦可）
cargo run -p scratch-card-server

# 测试与静态检查
cargo test
cargo clippy -- -D warnings

# 交叉编译并打包为树莓派发行包（在 WSL / Linux 中）
bash scripts/build.sh          # 产物输出到 dist/
```

### 可配置环境变量

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `SCRATCH_BIND` | `0.0.0.0:3000` | 监听地址；`0.0.0.0` 表示局域网可访问 |
| `SCRATCH_WEB_DIR` | `web` | 前端静态文件目录 |
| `SCRATCH_CONFIG` | `config/game.json` | 游戏配置文件路径 |
| `SCRATCH_RATE_LIMIT` | `120` | 每 IP 每 10 秒允许的请求数 |
| `RUST_LOG` | `info` | 日志级别：`error`/`warn`/`info`/`debug`/`trace` |

---

## 架构与目录结构

```text
.
├── backend/                 # Rust 服务端
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs          # 入口：读配置、建仓库、组装路由、监听端口
│       ├── api.rs           # HTTP 接口层 + 限流中间件 + 统一错误处理
│       ├── game.rs          # 核心逻辑：发牌、概率、三连判定、刮开、结算
│       ├── models.rs        # 数据结构与 API 报文定义
│       ├── config.rs        # game.json 的加载与校验
│       ├── state.rs         # 内存卡片仓库（HashMap + 清理策略）
│       └── ratelimit.rs     # 按 IP 的固定窗口限流器
├── web/                     # 前端静态文件（无构建步骤）
├── config/game.json         # 游戏规则配置
├── scripts/                 # build.sh / deploy.sh / check-pi.sh / e2e-browser.mjs
├── systemd/                 # systemd 服务单元模板
└── dist/                    # 构建产物（发行包，不纳入版本控制）
```

数据流：`浏览器 → HTTP → axum 路由 → api.rs handler → 读写 GameStore → 返回 JSON`。
所有卡片数据保存在内存中，**进程重启即清空**。

---

## 测试与验证

- **单元测试**：`cargo test`，覆盖发牌规则、卡片 ID 唯一性、概率分布、刮格边界、结算唯一性。
- **静态检查**：`cargo clippy -- -D warnings`，警告视为错误。
- **端到端测试**：`node scripts/e2e-browser.mjs`，用真实鼠标与触摸事件连抽三张卡，
  校验刮除、自动结算、中奖/未中奖展示、次数扣减与连抽。
- **部署冒烟**：`bash scripts/check-pi.sh <IP>:3000`，验证已部署服务的 LAN API。

---

## 已知限制与注意事项

1. **无持久化**：数据仅存内存，进程重启后所有卡片与统计清零；服务端也没有排行榜等跨会话数据。
2. **每日次数服务端不强制**（见上文 ⚠️），可被前端绕过。
3. **仅限 ARM64 Linux**：发行包二进制不能在 Windows / x86-64 上运行，需在树莓派上运行。
4. **单机内存存储**：卡片上限 2000 张，适合个人/小范围使用，不适合高并发大规模场景。
5. **非博彩用途**：本项目仅为技术演示与娱乐，不涉及真实货币，不得用于任何形式的赌博。

---

## 后续可扩展方向

- 服务端强制每日次数限制（按 IP / 设备记录）。
- 外部化统计：把累计发卡数、中奖数、发放积分持久化或做排行榜。
- 配置热加载（改 `game.json` 无需重启）。
- HTTPS 与反向代理，支持公网访问。

---

*本说明由项目实际代码（`backend/src/**`、`web/**`、`config/game.json`）整理生成，
配置数值以 `config/game.json` 为准。*
