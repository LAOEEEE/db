# 幸运刮刮乐（Rust + Axum + HTML/CSS/JS）

运行在树莓派 3B 上的轻量级刮刮乐网页应用：

- 后端：Rust + Axum，单二进制提供 API 和静态文件
- 前端：原生 HTML / CSS / JavaScript（Canvas 刮奖，无框架、无 Node 构建）
- 存储：内存（`Arc<RwLock<...>>`），无需数据库
- 中奖结果**只由后端决定**，前端只负责展示和交互

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
│   └── e2e-browser.mjs  # 无头浏览器端到端测试（鼠标 + 触摸）
└── systemd/scratch-card.service
```

## API

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET  | `/api/health` | 健康检查 |
| GET  | `/api/config` | 每日次数、刮开阈值（公开配置） |
| POST | `/api/game/new` | 新建刮刮卡，返回 `card_id`（不返回真实图案） |
| POST | `/api/game/reveal` | 揭示单格 `{card_id, cell_id}` → `{cell_id, symbol}` |
| POST | `/api/game/finish` | 结算，只能一次，返回中奖信息 |

刮奖动作完全在浏览器本地完成（Canvas 擦除），达到阈值后只发一次结算请求，
避免鼠标移动产生大量 HTTP 请求。

## 配置（config/game.json）

```json
{
  "rewards": [
    { "name": "一等奖", "symbol": "💎", "value": 1000, "probability": 0.001 },
    { "name": "二等奖", "symbol": "💰", "value": 100,  "probability": 0.01 },
    { "name": "三等奖", "symbol": "🎁", "value": 20,   "probability": 0.05 },
    { "name": "安慰奖", "symbol": "🍀", "value": 5,    "probability": 0.2 }
  ],
  "filler_symbols": ["⭐", "🍒", "🔔", "🍋", "🍇", "🍉"],
  "daily_limit": 10,
  "reveal_threshold": 0.4
}
```

概率之和不能超过 1，其余为未中奖。中奖卡保证有且仅有一条三连（横/竖/斜）。
`daily_limit` 为 0 表示不限次数；`reveal_threshold` 为自动结算的刮开面积比例，
满足"刮开面积达到阈值"**或**"9 个图案全部出现"任一条件即结算。
本项目是演示用小游戏，不是彩票或博彩系统。

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
cargo clippy
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
# 产物在 dist/：scratch-card-server、web/、config/、systemd unit
```

部署（树莓派开启 ssh，默认 laoeeee@192.168.1.246）：

```bash
wsl bash -lc 'cd /mnt/d/ydd_workspace/db && PI_HOST=laoeeee@192.168.1.246 bash scripts/deploy.sh'
```

脚本自动检测目标 sudo 能力：

- **无免密 sudo**（当前树莓派情况）：文件装到 `~/scratch-card/`，使用 **用户级 systemd**
  （`systemctl --user ...`），无需密码。首次部署后执行一次以实现断电开机自启：
  `ssh -t laoeeee@192.168.1.246 'sudo loginctl enable-linger laoeeee'`
- **有免密 sudo**：装到 `/opt/scratch-card/` 并安装系统级 unit（自动以登录用户运行）。

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

脚本用真实鼠标和触摸事件连抽三张卡，校验刮除、自动结算、中奖/未中奖展示、
次数扣减和连抽。把 `APP_URL` 指向已部署的树莓派即可做真机验证：

```bash
APP_URL=http://192.168.1.246:3000/ node scripts/e2e-browser.mjs
```
