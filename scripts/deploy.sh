#!/usr/bin/env bash
# Deploy dist/ to a Raspberry Pi over ssh/rsync and (re)start the service.
#
# Supports two modes, auto-detected on the target:
#   - system mode (passwordless sudo available): /opt/scratch-card + system unit
#   - user mode  (no passwordless sudo):          ~/scratch-card  + user unit
#
# Usage:
#   PI_HOST=laoeeee@192.168.1.246 ./scripts/deploy.sh
# Overrides: PI_HOST, PI_DIR (system mode only, default /opt/scratch-card)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PI_HOST="${PI_HOST:-laoeeee@192.168.1.246}"
PI_DIR="${PI_DIR:-/opt/scratch-card}"
DIST="$ROOT/dist"

if [ ! -f "$DIST/scratch-card-server" ]; then
    echo "error: dist/ not found, run scripts/build.sh first" >&2
    exit 1
fi

echo "==> checking target ${PI_HOST}"
if ssh "$PI_HOST" "sudo -n true" 2>/dev/null; then
    MODE="system"
else
    MODE="user"
fi
echo "==> deploy mode: ${MODE}"

if [ "$MODE" = "system" ]; then
    PI_USER="${PI_USER:-$(ssh "$PI_HOST" 'id -un')}"
    # 目录用 sudo 创建后属主是 root，而下面的 rsync 是以普通用户身份运行的，
    # 会因权限不足写不进去（Operation not permitted / Permission denied），
    # 所以建完目录立刻把属主改成登录用户（服务本身也以该用户运行）。
    ssh "$PI_HOST" "sudo mkdir -p ${PI_DIR}/web/assets ${PI_DIR}/config && sudo chown -R ${PI_USER}:${PI_USER} ${PI_DIR}"
    rsync -az --delete "$DIST/web/" "$PI_HOST:${PI_DIR}/web/"
    rsync -az "$DIST/config/game.json" "$PI_HOST:${PI_DIR}/config/game.json"
    scp "$DIST/scratch-card-server" "$PI_HOST:/tmp/scratch-card-server.new"
    ssh "$PI_HOST" "sudo mv /tmp/scratch-card-server.new ${PI_DIR}/scratch-card-server && sudo chmod +x ${PI_DIR}/scratch-card-server"

    sed "s/^User=.*/User=${PI_USER}/" "$DIST/scratch-card.service" > /tmp/scratch-card.service
    scp /tmp/scratch-card.service "$PI_HOST:/tmp/scratch-card.service"
    ssh "$PI_HOST" "sudo mv /tmp/scratch-card.service /etc/systemd/system/scratch-card.service && sudo systemctl daemon-reload && sudo systemctl enable scratch-card && sudo systemctl restart scratch-card"

    echo "==> status:"
    ssh "$PI_HOST" "systemctl --no-pager --full status scratch-card | head -12"
else
    UNIT="scratch-card.service"
    ssh "$PI_HOST" "mkdir -p ~/scratch-card/web/assets ~/scratch-card/config ~/.config/systemd/user"
    rsync -az --delete "$DIST/web/" "$PI_HOST:scratch-card/web/"
    rsync -az "$DIST/config/game.json" "$PI_HOST:scratch-card/config/game.json"
    scp "$DIST/scratch-card-server" "$PI_HOST:scratch-card/scratch-card-server.new"
    ssh "$PI_HOST" "chmod +x ~/scratch-card/scratch-card-server.new && mv ~/scratch-card/scratch-card-server.new ~/scratch-card/scratch-card-server"

    ssh "$PI_HOST" "cat > ~/.config/systemd/user/${UNIT}" <<'UNIT_EOF'
[Unit]
Description=Rust Scratch Card Web Server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=%h/scratch-card
ExecStart=%h/scratch-card/scratch-card-server
Restart=always
RestartSec=3

Environment=RUST_LOG=info
Environment=SCRATCH_BIND=0.0.0.0:3000
Environment=SCRATCH_WEB_DIR=%h/scratch-card/web
Environment=SCRATCH_CONFIG=%h/scratch-card/config/game.json

[Install]
WantedBy=default.target
UNIT_EOF

    ssh "$PI_HOST" "systemctl --user daemon-reload && systemctl --user enable ${UNIT} && systemctl --user restart ${UNIT}"

    echo "==> status:"
    ssh "$PI_HOST" "systemctl --user --no-pager --full status ${UNIT} | head -12"
    echo ""
    echo "==> boot persistence: run this once (asks for the Pi password):"
    echo "    ssh -t ${PI_HOST} 'sudo loginctl enable-linger '\$(whoami)"
fi
