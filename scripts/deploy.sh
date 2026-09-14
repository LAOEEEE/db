#!/usr/bin/env bash
# Deploy dist/ to a Raspberry Pi as a **user-level** systemd service.
#
# 只装用户级服务：文件放 ~/scratch-card，unit 放 ~/.config/systemd/user/。
# 全程不需要 root——不碰 /etc/、不碰 /opt/，也不需要任何 sudo 权限。
#
# 每次部署的顺序固定为：
#   停旧服务 -> 杀掉残留进程 -> 等端口释放 -> 传文件 -> 装 unit -> 启动 -> 报状态。
# 先停后传，是为了不出现"新二进制已经就位、旧进程还在跑旧代码"的中间状态；
# 端口那一步宁可多等几秒，也不带着已知冲突往下走。
#
# 【用户级服务的前提】要开机自启、并且在没人登录时也能继续运行，必须开一次 linger：
#   ssh -t <host> 'sudo loginctl enable-linger <user>'
# 这一步需要 root，但只是**一次性**的，不需要给部署用户常驻 sudo 权限。
# 没开 linger 时：服务只在该用户有活动会话时运行，重启/注销后不会自己起来。
# 脚本会检测并明确提示，但不会替你去开（因为那需要 root，而本脚本刻意不用 root）。
#
# Usage:
#   PI_HOST=laoeeee@192.168.1.246 ./scripts/deploy.sh
# Overrides: PI_HOST, PI_DIR（相对家目录，默认 scratch-card）, PI_PORT（默认 3000）
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PI_HOST="${PI_HOST:-laoeeee@192.168.1.246}"
PI_DIR="${PI_DIR:-scratch-card}"
PI_PORT="${PI_PORT:-3000}"
UNIT="scratch-card.service"
DIST="$ROOT/dist"

if [ ! -f "$DIST/scratch-card-server" ]; then
    echo "error: dist/ not found, run scripts/build.sh first" >&2
    exit 1
fi

PI_USER="${PI_USER:-$(ssh "$PI_HOST" 'id -un')}"
echo "==> target ${PI_HOST} (user=${PI_USER} dir=~/${PI_DIR} port=${PI_PORT})"

# 这个变量在下面反复出现：非交互 ssh 里的 systemctl --user 不知道去哪找用户级
# systemd 的 dbus，必须显式给 XDG_RUNTIME_DIR，否则一律报
# "Failed to connect to user scope bus via local transport"。
# export 一次，后面几条命令共用。
USERCTL="export XDG_RUNTIME_DIR=/run/user/\$(id -u); systemctl --user"

echo "==> checking user systemd availability"
if ssh "$PI_HOST" "$USERCTL list-units >/dev/null 2>&1"; then
    echo "    user systemd: ok"
else
    echo "warn: 连不上 ${PI_USER} 的用户级 systemd，下面的 systemctl --user 可能全部失败。" >&2
    echo "      该用户需要一个活动会话，或者已开启 linger：" >&2
    echo "      ssh -t ${PI_HOST} 'sudo loginctl enable-linger ${PI_USER}'" >&2
fi

# 如果 /etc/systemd/system 下还留着 system 级 unit，问题会非常隐蔽：
# 它是 enabled 的，会在我们杀掉进程后由 systemd 立刻把服务拉回来，
# 于是端口永远腾不出来，用户级服务永远起不来，而日志里只看到"Address already in use"。
# 所以这里直接拒绝往下走，并给出清掉它的命令——那一步需要 root，脚本自己不做。
if ssh "$PI_HOST" "systemctl is-enabled ${UNIT} >/dev/null 2>&1"; then
    echo "error: ${PI_HOST} 上还存在 system 级 unit ${UNIT}，会和用户级服务抢端口 ${PI_PORT}。" >&2
    echo "先清掉它（需要 root，一次性；之后就不需要 sudo 了）：" >&2
    echo "  ssh -t ${PI_HOST} 'sudo systemctl disable --now ${UNIT} && sudo rm /etc/systemd/system/${UNIT} && sudo systemctl daemon-reload'" >&2
    exit 1
fi

# ---- 1. 先停旧服务、杀掉残留进程，确保端口腾出来 ----
# 只 stop 服务不够：手工启动过、或者从别的目录（比如旧版 system 部署的 /opt）
# 跑起来的实例不受这个 unit 管理，stop 之后照样占着端口。
# pkill 的模式写成 scratch-card-serve[r]：这是正则，能匹配 scratch-card-server，
# 但它自己的命令行里是带方括号的字面量、不匹配该正则，所以不会误杀正在执行它的远程 shell。
echo "==> stopping existing service"
ssh "$PI_HOST" "$USERCTL stop ${UNIT} 2>/dev/null || true"
ssh "$PI_HOST" "pkill -f 'scratch-card-serve[r]' 2>/dev/null || true"

# 端口不会在 kill 的瞬间释放，等一下再确认。
for _ in 1 2 3 4 5; do
    if ! ssh "$PI_HOST" "ss -ltn 2>/dev/null | grep -q ':${PI_PORT} '"; then
        break
    fi
    sleep 1
done
if ssh "$PI_HOST" "ss -ltn 2>/dev/null | grep -q ':${PI_PORT} '"; then
    echo "error: ${PI_HOST}:${PI_PORT} 仍被占用，中止部署。当前占用者：" >&2
    ssh "$PI_HOST" "ss -ltnp 2>/dev/null | grep ':${PI_PORT} '" >&2
    exit 1
fi

# ---- 2. 传文件 ----
# 全是自己家目录下的路径，不需要 chown、不需要 sudo。
echo "==> syncing files to ~/${PI_DIR}"
ssh "$PI_HOST" "mkdir -p ~/${PI_DIR}/web/assets ~/${PI_DIR}/config ~/.config/systemd/user"
rsync -az --delete "$DIST/web/" "$PI_HOST:${PI_DIR}/web/"
rsync -az "$DIST/config/game.json" "$PI_HOST:${PI_DIR}/config/game.json"
# 先传成 .new 再就地改名：直接覆盖正在运行的二进制会得到 "Text file busy"。
scp "$DIST/scratch-card-server" "$PI_HOST:${PI_DIR}/scratch-card-server.new"
ssh "$PI_HOST" "chmod +x ~/${PI_DIR}/scratch-card-server.new && mv ~/${PI_DIR}/scratch-card-server.new ~/${PI_DIR}/scratch-card-server"

# ---- 3. 生成并安装 unit，然后启动 ----
# 用户级 unit 和 system 级不一样：没有 User=，路径用 %h（家目录）而不是写死 /opt，
# 挂到 default.target（而不是 multi-user.target）。
# SCRATCH_BIND 用的是上面那个 PI_PORT，端口只有这一个来源，
# 不会出现"unit 里写一个端口、脚本里检查另一个端口"的错位。
cat > "/tmp/$UNIT" <<EOF
[Unit]
Description=Rust Scratch Card Web Server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=%h/${PI_DIR}
ExecStart=%h/${PI_DIR}/scratch-card-server
Restart=always
RestartSec=3

Environment=RUST_LOG=info
Environment=SCRATCH_BIND=0.0.0.0:${PI_PORT}
Environment=SCRATCH_WEB_DIR=%h/${PI_DIR}/web
Environment=SCRATCH_CONFIG=%h/${PI_DIR}/config/game.json

[Install]
WantedBy=default.target
EOF
scp "/tmp/$UNIT" "$PI_HOST:.config/systemd/user/$UNIT"

echo "==> installing unit and starting service"
ssh "$PI_HOST" "$USERCTL daemon-reload && $USERCTL enable ${UNIT} && $USERCTL restart ${UNIT}"

# ---- 4. 报状态，并提醒 linger ----
echo "==> status:"
ssh "$PI_HOST" "$USERCTL --no-pager --full status ${UNIT} | head -12"

echo ""
if ssh "$PI_HOST" "loginctl show-user ${PI_USER} -p Linger 2>/dev/null | grep -q 'Linger=yes'"; then
    echo "==> linger: 已开启（重启后会自动起）"
else
    echo "==> linger: 未开启 —— 重启或注销后服务不会自动运行。开一次即可（需要 root）："
    echo "    ssh -t ${PI_HOST} 'sudo loginctl enable-linger ${PI_USER}'"
fi
