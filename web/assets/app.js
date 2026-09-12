'use strict';

(function () {
    const els = {
        board: document.getElementById('board'),
        cells: Array.from(document.querySelectorAll('.cell')),
        foil: document.getElementById('foil'),
        hint: document.getElementById('hint'),
        result: document.getElementById('result'),
        action: document.getElementById('action'),
        remaining: document.getElementById('remaining'),
        points: document.getElementById('points'),
        card: document.getElementById('card'),
    };

    const DEFAULT_CONFIG = { daily_limit: 0, reveal_threshold: 0.9 };
    const SAMPLE_GRID = 48;

    const config = { value: DEFAULT_CONFIG };
    const stats = loadStats();
    const state = {
        cardId: null,
        revealed: new Set(),
        pending: new Set(),
        drawing: false,
        completing: false,
        finished: false,
        lastCheck: 0,
        lastPoint: null,
        brush: 30,
        rect: null,
    };

    const api = {
        async request(path, body) {
            const opts = body
                ? {
                      method: 'POST',
                      headers: { 'Content-Type': 'application/json' },
                      body: JSON.stringify(body),
                  }
                : undefined;
            const res = await fetch(path, opts);
            const data = await res.json().catch(() => ({}));
            if (!res.ok) throw new Error(data.message || '服务器错误 (' + res.status + ')');
            return data;
        },
        config() {
            return this.request('/api/config');
        },
        newCard() {
            return this.request('/api/game/new', {});
        },
        reveal(cardId, cellId) {
            return this.request('/api/game/reveal', { card_id: cardId, cell_id: cellId });
        },
        finish(cardId) {
            return this.request('/api/game/finish', { card_id: cardId });
        },
    };

    function today() {
        const d = new Date();
        return d.getFullYear() + '-' + String(d.getMonth() + 1).padStart(2, '0') + '-' + String(d.getDate()).padStart(2, '0');
    }

    function loadStats() {
        try {
            const raw = JSON.parse(localStorage.getItem('scratch-stats') || 'null');
            if (raw && raw.date === today()) return raw;
        } catch (_) {}
        return { date: today(), remaining: null, points: 0 };
    }

    function saveStats() {
        try {
            localStorage.setItem('scratch-stats', JSON.stringify(stats));
        } catch (_) {}
    }

    const isUnlimited = () => (config.value.daily_limit || 0) <= 0;

    function renderStatus() {
        if (isUnlimited()) {
            els.remaining.textContent = '今日剩余：不限';
        } else {
            const left = stats.remaining === null ? '--' : stats.remaining;
            els.remaining.textContent = '今日剩余：' + left + ' 次';
        }
        els.points.textContent = '今日积分：' + stats.points;
        if (!isUnlimited() && stats.remaining !== null && stats.remaining <= 0 && !state.cardId) {
            els.action.disabled = true;
            els.action.textContent = '今日次数已用完';
        }
    }

    function showResult(text, kind) {
        els.result.textContent = text;
        els.result.className = 'result show ' + (kind || '');
    }

    function resetBoard() {
        for (const cell of els.cells) {
            cell.textContent = '';
            cell.className = 'cell';
        }
        els.result.className = 'result';
        els.result.textContent = '';
        els.card.classList.remove('win');
    }

    function setupFoil() {
        const canvas = els.foil;
        const rect = els.board.getBoundingClientRect();
        const dpr = Math.min(window.devicePixelRatio || 1, 2);
        canvas.width = Math.round(rect.width * dpr);
        canvas.height = Math.round(rect.height * dpr);
        const ctx = canvas.getContext('2d');
        ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

        const g = ctx.createLinearGradient(0, 0, rect.width, rect.height);
        g.addColorStop(0, '#c9a2ff');
        g.addColorStop(0.5, '#9a6fe0');
        g.addColorStop(1, '#7c4fc9');
        ctx.fillStyle = g;
        ctx.fillRect(0, 0, rect.width, rect.height);

        ctx.globalAlpha = 0.22;
        ctx.fillStyle = '#ffffff';
        ctx.font = 'bold ' + Math.round(rect.width / 9) + 'px sans-serif';
        ctx.textAlign = 'center';
        ctx.textBaseline = 'middle';
        for (let y = 5; y < 9; y += 2) {
            for (let x = 3; x < 10; x += 3) {
                ctx.fillText('刮', (rect.width / 10) * x, (rect.height / 10) * (y + 1));
            }
        }
        ctx.globalAlpha = 0.35;
        ctx.font = Math.round(rect.width / 7) + 'px sans-serif';
        ctx.fillText('✨  刮开好运  ✨', rect.width / 2, rect.height / 2);
        ctx.globalAlpha = 1;

        canvas.hidden = false;
        canvas.classList.remove('cleared');
        state.brush = Math.max(18, rect.width / 9.5);
        return rect;
    }

    function pointFromEvent(e) {
        const rect = state.rect;
        return { x: e.clientX - rect.left, y: e.clientY - rect.top };
    }

    function cellAt(point) {
        const cells = els.cells;
        for (let i = 0; i < cells.length; i++) {
            const r = cells[i].getBoundingClientRect();
            if (
                point.x + state.rect.left >= r.left &&
                point.x + state.rect.left <= r.right &&
                point.y + state.rect.top >= r.top &&
                point.y + state.rect.top <= r.bottom
            ) {
                return i;
            }
        }
        return -1;
    }

    function requestReveal(cellId) {
        if (state.revealed.has(cellId) || state.pending.has(cellId)) return;
        state.pending.add(cellId);
        api.reveal(state.cardId, cellId)
            .then((data) => {
                state.revealed.add(data.cell_id);
                const cell = els.cells[data.cell_id];
                cell.textContent = data.symbol;
                cell.classList.add('revealed');
                if (state.cardId && !state.drawing && !state.completing && shouldComplete()) {
                    completeCard();
                }
            })
            .catch(() => {})
            .finally(() => state.pending.delete(cellId));
    }

    function scratchTo(point) {
        const ctx = els.foil.getContext('2d');
        ctx.globalCompositeOperation = 'destination-out';
        ctx.lineWidth = state.brush;
        ctx.lineCap = 'round';
        ctx.lineJoin = 'round';
        ctx.beginPath();
        if (state.lastPoint) {
            ctx.moveTo(state.lastPoint.x, state.lastPoint.y);
            ctx.lineTo(point.x, point.y);
        } else {
            ctx.moveTo(point.x, point.y);
            ctx.lineTo(point.x + 0.01, point.y);
        }
        ctx.stroke();
        state.lastPoint = point;

        const id = cellAt(point);
        if (id >= 0) requestReveal(id);
    }

    let sampler = null;
    function scratchedRatio() {
        const source = els.foil;
        if (!sampler) sampler = document.createElement('canvas');
        sampler.width = SAMPLE_GRID;
        sampler.height = SAMPLE_GRID;
        const sctx = sampler.getContext('2d');
        sctx.clearRect(0, 0, SAMPLE_GRID, SAMPLE_GRID);
        sctx.drawImage(source, 0, 0, SAMPLE_GRID, SAMPLE_GRID);
        const data = sctx.getImageData(0, 0, SAMPLE_GRID, SAMPLE_GRID).data;
        let clear = 0;
        for (let i = 3; i < data.length; i += 4) {
            if (data[i] < 128) clear++;
        }
        return clear / (SAMPLE_GRID * SAMPLE_GRID);
    }

    async function completeCard() {
        if (state.completing) return;
        state.completing = true;
        state.drawing = false;
        els.foil.classList.add('cleared');

        const pendingReveals = [];
        for (let i = 0; i < 9; i++) {
            if (!state.revealed.has(i) && !state.pending.has(i)) {
                pendingReveals.push(
                    api
                        .reveal(state.cardId, i)
                        .then((data) => {
                            state.revealed.add(data.cell_id);
                            const cell = els.cells[data.cell_id];
                            cell.textContent = data.symbol;
                            cell.classList.add('revealed');
                        })
                        .catch(() => {})
                );
            }
        }
        await Promise.all(pendingReveals);

        try {
            const res = await api.finish(state.cardId);
            state.finished = true;
            showResult(res.message, res.win ? 'win' : 'lose');
            if (res.win) {
                els.card.classList.add('win');
                for (const cell of els.cells) {
                    if (cell.textContent === res.symbol) cell.classList.add('winning');
                }
                stats.points += res.reward;
                saveStats();
                renderStatus();
            }
        } catch (err) {
            showResult(err.message, 'error');
        }

        if (!isUnlimited() && stats.remaining !== null && stats.remaining <= 0) {
            els.action.disabled = true;
            els.action.textContent = '今日次数已用完';
        } else {
            els.action.disabled = false;
            els.action.textContent = '再来一张';
        }
    }

    // 结算条件：刮开面积达到阈值（默认 90%），或 9 个图案全部出现。
    function shouldComplete() {
        return (
            scratchedRatio() >= config.value.reveal_threshold ||
            state.revealed.size >= 9
        );
    }

    function onPointerDown(e) {
        if (!state.cardId || state.completing) return;
        e.preventDefault();
        state.drawing = true;
        state.lastPoint = null;
        state.rect = els.foil.getBoundingClientRect();
        els.foil.setPointerCapture(e.pointerId);
        scratchTo(pointFromEvent(e));
    }

    function onPointerMove(e) {
        if (!state.drawing) return;
        e.preventDefault();
        scratchTo(pointFromEvent(e));
        const now = performance.now();
        if (now - state.lastCheck > 180) {
            state.lastCheck = now;
            if (shouldComplete()) completeCard();
        }
    }

    function onPointerUp() {
        state.drawing = false;
        state.lastPoint = null;
        if (state.cardId && !state.completing && shouldComplete()) {
            completeCard();
        }
    }

    async function startCard() {
        if (state.cardId && !state.finished) return;
        els.action.disabled = true;
        els.action.textContent = '请稍候…';
        try {
            const card = await api.newCard();
            state.cardId = card.card_id;
            state.revealed.clear();
            state.pending.clear();
            state.completing = false;
            state.finished = false;
            resetBoard();
            els.hint.hidden = true;
            state.rect = setupFoil();

            if (!isUnlimited()) {
                if (stats.remaining === null) stats.remaining = config.value.daily_limit;
                stats.remaining = Math.max(0, stats.remaining - 1);
                saveStats();
            }
            renderStatus();

            els.action.disabled = true;
            els.action.textContent = '刮奖中…';
        } catch (err) {
            els.hint.hidden = false;
            showResult(err.message, 'error');
            els.action.disabled = false;
            els.action.textContent = '开始刮奖';
        }
    }

    els.foil.addEventListener('pointerdown', onPointerDown);
    els.foil.addEventListener('pointermove', onPointerMove);
    els.foil.addEventListener('pointerup', onPointerUp);
    els.foil.addEventListener('pointercancel', onPointerUp);
    els.action.addEventListener('click', startCard);

    api.config()
        .then((cfg) => {
            config.value = cfg;
            if (!isUnlimited() && stats.remaining === null) stats.remaining = cfg.daily_limit;
            renderStatus();
        })
        .catch(() => {
            if (!isUnlimited()) stats.remaining = DEFAULT_CONFIG.daily_limit;
            renderStatus();
            showResult('无法连接服务器，请稍后重试', 'error');
        });

    renderStatus();
})();
