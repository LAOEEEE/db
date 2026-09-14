# 幸运刮刮乐（Rust + Axum + HTML/CSS/JS）

运行在树莓派 3B 上的轻量级刮刮乐网页应用：

> 当前版本 **v1.0.0**（2026-09-11）

- 后端：Rust + Axum，单二进制提供 API 和静态文件
- 前端：原生 HTML / CSS / JavaScript（Canvas 刮奖，无框架、无 Node 构建）
- 存储：内存（`Arc<RwLock<...>>`），无需数据库
- 中奖结果**只由后端决定**，前端只负责展示和交互

## 玩法

- 进入页面先设置**这一次要刮几次**（默认 10 次）
- 一页最多 5 个刮奖格，每格单独刮；页面上一直显示 **未刮 / 已刮 / 第几页**
- 刮开一格就是开奖一次：中奖显示 `$20` 这样的金额，并且这一格背景整块变黄；没中显示 `$0`
- 一页 5 格刮完自动结算，还有剩余次数就点「进入下一页」继续刮剩下的

## 目录结构

```text
.
├── backend/            # Axum 服务端
│   └── src/
│       ├── main.rs      # 启动、路由、静态文件
│       ├── api.rs       # HTTP handler、错误处理、限流
│       ├── game.rs      # 发牌、概率、中奖判定
│       ├── models.rs    # 数据模型与 API 报文
│       ├── config.rs    # config/game.json 加载与校验
│       ├── state.rs     # 内存游戏存储
│       └── ratelimit.rs # 按 IP 的简单限流
├── web/                 # 前端静态文件（index.html + assets/）
├── config/game.json     # 奖级、概率、每日次数、刮开阈值
├── scripts/
│   ├── build.sh         # WSL 内交叉编译 aarch64 并打包 dist/
│   ├── deploy.sh        # rsync/scp 部署到树莓派并重启服务
│   ├── check-pi.sh      # 部署后对 LAN API 做冒烟检查
│   └── e2e-browser.mjs  # 无头浏览器端到端测试（鼠标 + 触摸）
├── dist/                # 构建产物（已 gitignore）
```

## API

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET  | `/api/health` | 健康检查 |
| GET  | `/api/config` | 一页格数、次数上限、刮开阈值、未中奖文本（公开配置） |
| POST | `/api/game/new` | 新建**一页**刮刮卡，可选 `{"count": 5}`，返回 `card_id` + 格子编号（不返回金额） |
| POST | `/api/game/reveal` | 刮开一格 `{card_id, cell_id}` → `{cell_id, amount, label, win}` |
| POST | `/api/game/finish` | 结算这一页（只能一次）→ `{win, reward, message}` |

刮奖动作（Canvas 擦除）完全在浏览器本地完成：每一格刮到 `reveal_threshold` 面积才发一次
`/api/game/reveal`，一页全部刮完才发一次 `/api/game/finish`，
鼠标移动本身不会产生任何 HTTP 请求。

每一格中多少钱，在服务器发牌那一刻就已经定死；前端拿不到"没刮开的那一格值多少钱"。

## 配置（config/game.json）

下面是当前仓库中 `config/game.json` 的实际内容：

```json
{
  "rewards": [
    { "name": "特等奖", "symbol": "$1000", "value": 1000, "probability": 0.001 },
    { "name": "一等奖", "symbol": "$100", "value": 100, "probability": 0.01 },
    { "name": "二等奖", "symbol": "$20", "value": 20, "probability": 0.05 },
    { "name": "三等奖", "symbol": "$5", "value": 5, "probability": 0.2 }
  ],
  "page_size": 5,
  "max_count": 100,
  "lose_label": "$0",
  "daily_limit": 0,
  "reveal_threshold": 0.7
}
```

| 字段 | 说明 | 约束 / 默认值 |
| --- | --- | --- |
| `rewards` | 奖级列表（名称 / 中奖显示的金额文本 / 金额 / 概率） | 不能为空；概率 0~1，所有概率之和 ≤ 1，其余为未中奖 |
| `page_size` | 一页最多几个刮奖格 | `1 ~ 5`，缺省 5 |
| `max_count` | 用户最多能设置多少次刮奖 | `1 ~ 10000` 且不小于 `page_size`，缺省 100 |
| `lose_label` | 没中奖时格子显示的文本 | 不能为空，缺省 `"$0"` |
| `daily_limit` | 次数上限兜底，**0 表示不限** | 非 0 时用户在设置页最多只能填这么多，缺省为 10 |
| `reveal_threshold` | 刮开面积达到此比例就算刮开这一格 | 必须在 `0.05 ~ 1.0`，缺省为 0.7 |
| `filler_symbols` | **旧版字段**（3×3 三连玩法用的填充符号） | 已不生效，配置里缺失也能启动。**别从结构体里删掉它**：结构体开了 `deny_unknown_fields`，删掉后仍带此键的旧 `game.json` 会解析失败、服务起不来 |

每一页最多 5 格，每格的奖级是**独立**按概率表抽的（一页里可能好几格同时中奖，也可能全没中）。
刮开面积达到 `reveal_threshold` 就算这一格开奖；中奖格显示 `symbol`（如 `$20`）且背景变黄，
没中奖的格子显示 `lose_label`（如 `$0`）。
本项目是演示用小游戏，不是彩票或博彩系统。

> ⚠️ **注意**：`daily_limit` 只在**浏览器端**限制用户在设置页能填多少，服务端并不强制。
> 清空缓存、换浏览器或直接调接口都能绕过，不具备防刷能力。若要真正限制次数需改后端。

## 本地开发（任意平台）

```bash
cargo run -p scratch-card-server
# 打开 http://localhost:3000
```

环境变量：`SCRATCH_BIND`（默认 `0.0.0.0:3000`）、`SCRATCH_WEB_DIR`、
`SCRATCH_CONFIG`、`SCRATCH_RATE_LIMIT`（每 10 秒每 IP 请求数，默认 120）、
`RUST_LOG`。

测试与检查：

```bash
cargo test
cargo clippy -- -D warnings
```

前端无构建步骤，直接修改 `web/` 下文件刷新即可。

## 树莓派部署（Windows + WSL 交叉编译）

WSL 内需安装：

```bash
rustup target add aarch64-unknown-linux-gnu
sudo apt install gcc-aarch64-linux-gnu rsync
```

构建并打包：

```bash
wsl bash -lc 'cd /mnt/d/ydd_workspace/db && bash scripts/build.sh'
# 产物组装到 dist/：scratch-card-server、web/、config/
# 以及压缩包 scratch-card-v<版本>.tar.gz（仅含上述运行文件，不含发行说明）
# （版本取自 Cargo.toml，可用 VERSION=... 环境变量覆盖）
```

部署（树莓派开启 ssh，默认 laoeeee@192.168.1.246）：

```bash
wsl bash -lc 'cd /mnt/d/ydd_workspace/db && PI_HOST=laoeeee@192.168.1.246 bash scripts/deploy.sh'
```

采用**用户级 systemd**：文件装到 `~/scratch-card/`，unit 由部署脚本就地生成到
`~/.config/systemd/user/`，全程不需要 sudo。部署顺序是"先停旧服务、清掉残留进程、
等端口释放，再传文件、装 unit、启动"，避免出现新旧进程抢同一端口的中间状态。

首次部署后执行一次以实现断电开机自启（需要 root，一次性；之后不再需要 sudo）：

```bash
ssh -t laoeeee@192.168.1.246 'sudo loginctl enable-linger laoeeee'
```

部署后：

```bash
# 用户级（当前模式）
ssh laoeeee@192.168.1.246 'systemctl --user status scratch-card'
ssh laoeeee@192.168.1.246 'journalctl --user -u scratch-card -f'
# 浏览器访问 http://192.168.1.246:3000
```

快速验证已部署服务（LAN API 冒烟）：

```bash
wsl bash -lc 'cd /mnt/d/ydd_workspace/db && bash scripts/check-pi.sh 192.168.1.246:3000'
```

## 浏览器端到端测试

需要本机 Edge/Chrome，服务运行在 127.0.0.1:3100：

```bash
SCRATCH_BIND=127.0.0.1:3100 cargo run -p scratch-card-server
node scripts/e2e-browser.mjs
```

脚本用真实的鼠标和触摸事件走完整流程：设置 10 次 → 一页 5 格逐格刮开 →
校验未刮/已刮计数、金额文本、中奖格黄色高亮、自动结算 → 点「进入下一页」再刮剩下 5 次。
需要 Node 18+（用到了全局 `fetch` / `WebSocket`）。把 `APP_URL` 指向已部署的树莓派即可做真机验证：

```bash
APP_URL=http://192.168.1.246:3000/ node scripts/e2e-browser.mjs
```
