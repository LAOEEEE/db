'use strict';

// 幸运刮刮乐 —— 前端交互
//
// 玩法：
//   一页 5 个刮奖格，每格**独立**刮开（Canvas 擦除涂层）
//   擦开面积达到阈值 → 请求 /api/game/reveal 拿这一格的金额
//   金额 > 0 → 这一格背景整块变黄，并飘出 "+$20"
//   一页全部刮开 → /api/game/finish 结算 → 还有剩余次数就点「进入下一页」
//   全程显示 未刮 / 已刮 / 第几页，次数由用户在设置页自己定
//
// 性能上有几个关键点（以后改动请保留）：
//   1. 5 个刮奖格和 Canvas 只创建一次，翻页时复用，不反复增删 DOM
//   2. 涂层图案预渲染到离屏 Canvas，重置格子只要 drawImage 一次
//   3. pointermove 用 requestAnimationFrame 合并，避免高频事件里重复画线
//   4. 擦除比例采样最多每 140ms 一次，且复用同一块 40×40 采样 Canvas
//   5. 只在每笔刮动开始时读一次 getBoundingClientRect，移动过程完全不碰布局

(function () {
    const MAX_SLOTS = 5;              // 和后端 models.rs 的 MAX_PAGE_CELLS 保持一致
    const SAMPLE_GRID = 40;           // 采样画布边长：40×40 判断比例足够，开销极低
    const RATIO_CHECK_MS = 140;       // 两次采样之间的最小间隔
    const MIN_THRESHOLD = 0.35;       // 刮开阈值的下限（防止配置写得太小一点就开奖）
    const MAX_THRESHOLD = 0.95;       // 刮开阈值的上限（保证总能刮开）
    const TODAY_KEY = 'scratch-stats-v2';

    const DEFAULT_CONFIG = {
        daily_limit: 0,
        reveal_threshold: 0.7,
        page_size: MAX_SLOTS,
        max_count: 100,
        lose_label: '$0',
    };

    const els = {
        setup: document.getElementById('setup'),
        game: document.getElementById('game'),
        subtitle: document.getElementById('subtitle'),
        chips: document.getElementById('chips'),
        countInput: document.getElementById('countInput'),
        minus: document.getElementById('minus'),
        plus: document.getElementById('plus'),
        start: document.getElementById('start'),
        setupTip: document.getElementById('setupTip'),
        board: document.getElementById('board'),
        leftNum: document.getElementById('leftNum'),
        doneNum: document.getElementById('doneNum'),
        pageNum: document.getElementById('pageNum'),
        result: document.getElementById('result'),
        pageWin: document.getElementById('pageWin'),
        totalWin: document.getElementById('totalWin'),
        action: document.getElementById('action'),
        skipPage: document.getElementById('skipPage'),
        restart: document.getElementById('restart'),
        footNote: document.getElementById('footNote'),
    };

    // ---- 全局状态 ----
    const config = Object.assign({}, DEFAULT_CONFIG);
    const stats = loadStats();       // 今日累计中奖金额（localStorage）

    const state = {
        gen: 0,            // "这一轮"的代号：翻页/重开都会 +1，用来丢弃过期请求的结果
        phase: 'setup',    // setup | loading | playing | settling | pageDone | allDone | error
        total: 0,          // 本轮设置的总刮奖次数
        done: 0,           // 已经刮开的次数（跨页累计）
        page: 0,           // 当前第几页（从 1 开始）
        pages: 1,          // 一共几页
        pageSlots: 0,      // 当前页实际有几个刮奖格
        pageDone: 0,       // 当前页已经刮开几个
        pageWon: 0,        // 当前页中奖金额
        sessionWon: 0,     // 本轮中奖金额
        cardId: null,      // 当前页的卡片 id
    };

    const slots = [];          // 复用的 5 个刮奖格
    let sampler = null;        // 复用的采样 Canvas
    let foilPattern = null;    // 预渲染的涂层图案
    let foilPatternKey = '';
    let resizeTimer = 0;

    // ---------------- 小工具 ----------------

    function today() {
        const d = new Date();
        return d.getFullYear() + '-' + String(d.getMonth() + 1).padStart(2, '0') + '-' +
            String(d.getDate()).padStart(2, '0');
    }

    function loadStats() {
        try {
            const raw = JSON.parse(localStorage.getItem(TODAY_KEY) || 'null');
            if (raw && raw.date === today() && typeof raw.won === 'number') return raw;
        } catch (_) {}
        return { date: today(), won: 0 };
    }

    function saveStats() {
        try { localStorage.setItem(TODAY_KEY, JSON.stringify(stats)); } catch (_) {}
    }

    function clamp(n, lo, hi) {
        return n < lo ? lo : n > hi ? hi : n;
    }

    function threshold() {
        const t = Number(config.reveal_threshold);
        return Number.isFinite(t) ? clamp(t, MIN_THRESHOLD, MAX_THRESHOLD) : DEFAULT_CONFIG.reveal_threshold;
    }

    function pageSize() {
        const n = Math.floor(Number(config.page_size) || MAX_SLOTS);
        return Math.round(clamp(n, 1, MAX_SLOTS));
    }

    function maxSettable() {
        const max = Math.floor(Number(config.max_count)) || DEFAULT_CONFIG.max_count;
        const daily = Math.floor(Number(config.daily_limit)) || 0;
        return Math.max(1, daily > 0 ? Math.min(max, daily) : max);
    }

    // ---------------- 网络请求 ----------------

    const api = {
        async request(path, body) {
            const opts = body === undefined ? undefined : {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(body),
            };
            let res;
            try {
                res = await fetch(path, opts);
            } catch (_) {
                throw new Error('连接不上服务器，请检查网络');
            }
            let data = {};
            try { data = await res.json(); } catch (_) {}
            if (!res.ok) {
                throw new Error(data && data.message ? data.message : '服务器错误 (' + res.status + ')');
            }
            return data;
        },
        config() { return this.request('/api/config'); },
        newCard(count) { return this.request('/api/game/new', { count: count }); },
        reveal(cardId, cellId) {
            return this.request('/api/game/reveal', { card_id: cardId, cell_id: cellId });
        },
        finish(cardId) { return this.request('/api/game/finish', { card_id: cardId }); },
    };

    // ---------------- DOM：界面文字 ----------------

    function showResult(text, kind) {
        els.result.textContent = text;
        els.result.className = 'result' + (text ? ' show ' : ' ') + (kind || '');
    }

    function setAction(text, disabled) {
        els.action.textContent = text;
        els.action.disabled = !!disabled;
    }

    function renderCounters() {
        els.leftNum.textContent = String(Math.max(0, state.total - state.done));
        els.doneNum.textContent = String(state.done);
        els.pageNum.textContent = state.page + '/' + state.pages;
    }

    function renderTotals() {
        els.pageWin.textContent = '$' + state.pageWon;
        els.totalWin.textContent = '$' + stats.won;
    }

    // ---------------- DOM：刮奖格 ----------------

    function buildSlots() {
        const frag = document.createDocumentFragment();
        for (let i = 0; i < MAX_SLOTS; i++) {
            const el = document.createElement('div');
            el.className = 'slot';
            el.hidden = true;

            const face = document.createElement('div');
            face.className = 'face';
            const amount = document.createElement('span');
            amount.className = 'amount';
            face.appendChild(amount);

            const canvas = document.createElement('canvas');
            canvas.className = 'foil';
            canvas.setAttribute('aria-hidden', 'true');

            const tag = document.createElement('span');
            tag.className = 'tag';
            tag.textContent = String(i + 1);

            el.appendChild(face);
            el.appendChild(canvas);
            el.appendChild(tag);
            frag.appendChild(el);

            const slot = {
                index: i,
                el: el,
                amountEl: amount,
                canvas: canvas,
                ctx: canvas.getContext('2d', { alpha: true, desynchronized: true }),
                dpr: 1,
                cssW: 0,
                cssH: 0,
                brush: 28,
                rect: null,
                id: i,
                revealed: false,
                revealing: false,
                active: false,
                hasPending: false,
                pendingX: 0,
                pendingY: 0,
                lastX: null,
                lastY: null,
                rafId: 0,
                lastCheck: 0,
                ratio: 0,
            };
            bindSlot(slot);
            slots.push(slot);
        }
        els.board.appendChild(frag);
    }

    function bindSlot(slot) {
        const canvas = slot.canvas;
        canvas.addEventListener('pointerdown', function (e) { onDown(slot, e); });
        canvas.addEventListener('pointermove', function (e) { onMove(slot, e); });
        canvas.addEventListener('pointerup', function (e) { onUp(slot, e); });
        canvas.addEventListener('pointercancel', function (e) { onUp(slot, e); });
        canvas.addEventListener('contextmenu', function (e) { e.preventDefault(); });
    }

    function resetSlot(slot) {
        if (slot.rafId) { cancelAnimationFrame(slot.rafId); slot.rafId = 0; }
        slot.revealed = false;
        slot.revealing = false;
        slot.active = false;
        slot.hasPending = false;
        slot.lastX = null;
        slot.lastY = null;
        slot.ratio = 0;
        slot.lastCheck = 0;
        slot.el.className = 'slot';
        slot.amountEl.textContent = '';
        slot.amountEl.classList.remove('lose');
        slot.canvas.classList.remove('cleared');
        paintFoil(slot);
    }

    // ---------------- Canvas：涂层与擦除 ----------------

    function foilPatternFor(w, h, dpr) {
        const key = w + 'x' + h + '@' + dpr;
        if (foilPattern && foilPatternKey === key) return foilPattern;
        foilPattern = buildFoilPattern(w, h, dpr);
        foilPatternKey = key;
        return foilPattern;
    }

    // 把"刮刮层"画到一块离屏 Canvas 上。同一尺寸只会画一次，之后都是 drawImage。
    function buildFoilPattern(w, h, dpr) {
        const c = document.createElement('canvas');
        c.width = Math.max(1, Math.round(w * dpr));
        c.height = Math.max(1, Math.round(h * dpr));
        const ctx = c.getContext('2d');
        ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

        const g = ctx.createLinearGradient(0, 0, w, h);
        g.addColorStop(0, '#c9a2ff');
        g.addColorStop(0.5, '#9a6fe0');
        g.addColorStop(1, '#7c4fc9');
        ctx.fillStyle = g;
        ctx.fillRect(0, 0, w, h);

        // 斜纹
        ctx.globalAlpha = 0.12;
        ctx.strokeStyle = '#ffffff';
        ctx.lineWidth = 2;
        ctx.beginPath();
        for (let x = -h; x < w + h; x += 16) {
            ctx.moveTo(x, h);
            ctx.lineTo(x + h, 0);
        }
        ctx.stroke();

        // "刮"字水印
        const fs = Math.max(16, Math.round(h * 0.6));
        ctx.globalAlpha = 0.2;
        ctx.fillStyle = '#ffffff';
        ctx.font = '700 ' + fs + 'px "PingFang SC","Microsoft YaHei",sans-serif';
        ctx.textAlign = 'center';
        ctx.textBaseline = 'middle';
        const step = Math.max(60, fs * 3);
        for (let x = step / 2; x < w + step; x += step) ctx.fillText('刮', x, h / 2);

        ctx.globalAlpha = 1;
        return c;
    }

    function paintFoil(slot) {
        if (!slot.cssW || !slot.cssH) return;
        const pattern = foilPatternFor(slot.cssW, slot.cssH, slot.dpr);
        const ctx = slot.ctx;
        ctx.globalCompositeOperation = 'source-over';
        ctx.setTransform(1, 0, 0, 1, 0, 0);
        ctx.clearRect(0, 0, slot.canvas.width, slot.canvas.height);
        ctx.drawImage(pattern, 0, 0);
        ctx.setTransform(slot.dpr, 0, 0, slot.dpr, 0, 0);
    }

    function measureSlots(count) {
        const dpr = Math.min(window.devicePixelRatio || 1, 2);
        for (let i = 0; i < count; i++) {
            const slot = slots[i];
            const rect = slot.canvas.getBoundingClientRect();
            if (rect.width < 1 || rect.height < 1) continue;
            slot.rect = rect;
            const w = Math.round(rect.width);
            const h = Math.round(rect.height);
            if (w !== slot.cssW || h !== slot.cssH || dpr !== slot.dpr) {
                slot.cssW = w;
                slot.cssH = h;
                slot.dpr = dpr;
                slot.canvas.width = Math.round(w * dpr);
                slot.canvas.height = Math.round(h * dpr);
                slot.brush = Math.max(16, Math.round(h * 0.5));
                slot.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
            }
            paintFoil(slot);
        }
    }

    function strokeTo(slot, x, y) {
        const ctx = slot.ctx;
        ctx.globalCompositeOperation = 'destination-out';
        ctx.lineWidth = slot.brush;
        ctx.lineCap = 'round';
        ctx.lineJoin = 'round';
        ctx.beginPath();
        if (slot.lastX === null) {
            // 只是点一下：用一条极短的线画出个圆点
            ctx.moveTo(x, y);
            ctx.lineTo(x + 0.01, y);
        } else {
            ctx.moveTo(slot.lastX, slot.lastY);
            ctx.lineTo(x, y);
        }
        ctx.stroke();
        slot.lastX = x;
        slot.lastY = y;
    }

    // 采样擦除比例：把涂层缩到 40×40 再看有多少像素是透明的。
    // 比逐像素扫描整块 Canvas 快得多，而且开销和格子大小无关。
    function scratchedRatio(slot) {
        if (!sampler) {
            sampler = document.createElement('canvas');
            sampler.width = SAMPLE_GRID;
            sampler.height = SAMPLE_GRID;
        }
        const sctx = sampler.getContext('2d');
        sctx.clearRect(0, 0, SAMPLE_GRID, SAMPLE_GRID);
        sctx.drawImage(slot.canvas, 0, 0, SAMPLE_GRID, SAMPLE_GRID);
        const data = sctx.getImageData(0, 0, SAMPLE_GRID, SAMPLE_GRID).data;
        let clear = 0;
        for (let i = 3; i < data.length; i += 4) {
            if (data[i] < 128) clear++;
        }
        return clear / (SAMPLE_GRID * SAMPLE_GRID);
    }

    // ---------------- 刮奖手势 ----------------

    function onDown(slot, e) {
        if (slot.revealed || slot.revealing || state.phase !== 'playing') return;
        e.preventDefault();
        // 上次刮够了但开奖请求失败：再点一下就是重试
        if (slot.ratio >= threshold()) { completeSlot(slot); return; }
        slot.rect = slot.canvas.getBoundingClientRect(); // 每笔刮动只读一次布局
        slot.active = true;
        slot.lastX = null;
        slot.lastY = null;
        try { slot.canvas.setPointerCapture(e.pointerId); } catch (_) {}
        strokeTo(slot, e.clientX - slot.rect.left, e.clientY - slot.rect.top);
    }

    function onMove(slot, e) {
        if (!slot.active) return;
        e.preventDefault();
        slot.pendingX = e.clientX - slot.rect.left;
        slot.pendingY = e.clientY - slot.rect.top;
        slot.hasPending = true;
        if (slot.rafId) return;
        slot.rafId = requestAnimationFrame(function () {
            slot.rafId = 0;
            if (!slot.active) return;
            if (slot.hasPending) {
                slot.hasPending = false;
                strokeTo(slot, slot.pendingX, slot.pendingY);
            }
            const now = performance.now();
            if (now - slot.lastCheck >= RATIO_CHECK_MS) {
                slot.lastCheck = now;
                // 顺手记下比例：万一开奖请求失败，用户再点一下就能重试
                slot.ratio = scratchedRatio(slot);
                if (slot.ratio >= threshold()) completeSlot(slot);
            }
        });
    }

    function onUp(slot, e) {
        if (!slot.active) return;
        endStroke(slot);
        if (e && e.pointerId !== undefined) {
            try { slot.canvas.releasePointerCapture(e.pointerId); } catch (_) {}
        }
        if (slot.revealed || slot.revealing) return;
        slot.ratio = scratchedRatio(slot);
        if (slot.ratio >= threshold()) completeSlot(slot);
    }

    // 结束一笔刮动：把 rAF 里还没画的最后一个点补上
    function endStroke(slot) {
        slot.active = false;
        if (slot.rafId) { cancelAnimationFrame(slot.rafId); slot.rafId = 0; }
        if (slot.hasPending) {
            slot.hasPending = false;
            strokeTo(slot, slot.pendingX, slot.pendingY);
        }
        slot.lastX = null;
        slot.lastY = null;
    }

    function endAllStrokes() {
        for (let i = 0; i < slots.length; i++) {
            if (slots[i].active) endStroke(slots[i]);
        }
    }

    // ---------------- 开奖 ----------------

    function completeSlot(slot) {
        if (slot.revealed || slot.revealing || state.phase !== 'playing') return;
        slot.revealing = true;
        slot.active = false;
        if (slot.rafId) { cancelAnimationFrame(slot.rafId); slot.rafId = 0; }
        slot.lastX = null;
        slot.lastY = null;
        slot.canvas.classList.add('cleared');   // 涂层淡出，露出下面的金额

        const gen = state.gen;
        api.reveal(state.cardId, slot.id).then(function (data) {
            if (gen !== state.gen) return;
            slot.revealing = false;
            applyReveal(slot, data);
        }).catch(function (err) {
            if (gen !== state.gen) return;
            // 开奖失败：把涂层放回来，让用户可以再刮/再点一次重试
            slot.revealing = false;
            slot.canvas.classList.remove('cleared');
            showResult((err && err.message) || '开奖失败，再刮一下试试', 'error');
        });
    }

    function applyReveal(slot, data) {
        if (slot.revealed) return;
        slot.revealed = true;
        slot.amountEl.textContent = data.label;
        slot.el.classList.add('revealed');

        if (data.win) {
            // 中奖格：背景变黄 + 飘出金额
            slot.el.classList.add('win');
            state.pageWon += data.amount;
            state.sessionWon += data.amount;
            stats.won += data.amount;
            saveStats();
            floatPrize(slot, data.label);
        } else {
            slot.amountEl.classList.add('lose');
        }

        state.done += 1;
        state.pageDone += 1;
        renderCounters();
        renderTotals();

        if (state.pageDone >= state.pageSlots) finishPage();
    }

    function floatPrize(slot, label) {
        const tip = document.createElement('span');
        tip.className = 'float-prize';
        tip.textContent = '+' + label;
        slot.el.appendChild(tip);
        const drop = function () {
            if (tip.parentNode) tip.parentNode.removeChild(tip);
        };
        tip.addEventListener('animationend', drop, { once: true });
        setTimeout(drop, 1500);   // 兜底：动画事件丢了也不会残留节点
    }

    // ---------------- 一页的开始 / 结算 / 翻页 ----------------

    function prepareSlots(count) {
        for (let i = 0; i < MAX_SLOTS; i++) {
            const slot = slots[i];
            const on = i < count;
            slot.el.hidden = !on;
            if (on) {
                slot.id = i;
                resetSlot(slot);
            }
        }
        measureSlots(count);   // 显示出来之后统一量一次尺寸
    }

    async function startPage() {
        if (state.phase === 'loading') return;
        const remaining = state.total - state.done;
        const cells = Math.round(clamp(remaining, 1, pageSize()));
        const gen = state.gen;

        state.phase = 'loading';
        state.page += 1;
        state.pageSlots = cells;
        state.pageDone = 0;
        state.pageWon = 0;
        state.cardId = null;
        setAction('发牌中…', true);
        prepareSlots(cells);
        renderCounters();
        renderTotals();
        showResult('刮开涂层，看看这一页的手气', '');

        try {
            const card = await api.newCard(cells);
            if (gen !== state.gen) return;
            state.cardId = card.card_id;
            state.phase = 'playing';
            setAction('刮开上面的格子', true);
        } catch (err) {
            if (gen !== state.gen) return;
            state.page -= 1;          // 没发成功就不算翻页
            state.phase = 'error';
            showResult((err && err.message) || '发牌失败，请重试', 'error');
            setAction('重试本页', false);
        }
    }

    function startRound(count) {
        state.gen += 1;
        state.total = count;
        state.done = 0;
        state.page = 0;
        state.pages = Math.max(1, Math.ceil(count / pageSize()));
        state.pageDone = 0;
        state.pageWon = 0;
        state.sessionWon = 0;
        state.cardId = null;
        els.setup.hidden = true;
        els.game.hidden = false;
        els.subtitle.textContent = '共 ' + count + ' 次 · 每页 ' + pageSize() + ' 次';
        els.footNote.textContent = '刮开面积达标自动开奖';
        renderCounters();
        renderTotals();
        startPage();
    }

    function finishPage() {
        if (state.phase !== 'playing') return;
        state.phase = 'settling';
        const gen = state.gen;
        api.finish(state.cardId).then(function (res) {
            if (gen !== state.gen) return;
            showResult(res.message, res.win ? 'win' : 'lose');
            pageFinished();
        }).catch(function (err) {
            if (gen !== state.gen) return;
            // 结算失败也不卡住用户：金额在刮开时已经按服务器返回记过了
            showResult((err && err.message) || '结算失败', 'error');
            pageFinished();
        });
    }

    function pageFinished() {
        const left = state.total - state.done;
        if (left > 0) {
            state.phase = 'pageDone';
            setAction('进入下一页（还剩 ' + left + ' 次）', false);
            els.footNote.textContent = '这一页刮完了，点按钮继续下一页';
        } else {
            state.phase = 'allDone';
            setAction('重新设置次数', false);
            showResult('🎊 本轮结束：刮开 ' + state.done + ' 次，共中奖 $' + state.sessionWon,
                state.sessionWon > 0 ? 'win' : 'lose');
            els.footNote.textContent = '本轮结束，可以再来一轮';
        }
    }

    function onAction() {
        if (state.phase === 'pageDone' || state.phase === 'error') startPage();
        else if (state.phase === 'allDone') backToSetup();
    }

    function skipPage() {
        if (state.phase !== 'playing') return;
        for (let i = 0; i < state.pageSlots; i++) {
            const slot = slots[i];
            if (slot.revealed || slot.revealing) continue;
            slot.ratio = 1;
            completeSlot(slot);
        }
    }

    function backToSetup() {
        state.gen += 1;        // 让还在路上的请求结果作废
        state.phase = 'setup';
        state.cardId = null;
        els.game.hidden = true;
        els.setup.hidden = false;
        els.subtitle.textContent = '先设置次数，再开刮';
        els.footNote.textContent = '刮开面积达标自动开奖';
        showResult('', '');
        setCount(currentCount(), false);
    }

    // ---------------- 设置页 ----------------

    function currentCount() {
        const n = parseInt(els.countInput.value, 10);
        return clamp(Number.isFinite(n) ? n : 10, 1, maxSettable());
    }

    function setCount(n, keepText) {
        const max = maxSettable();
        const v = Math.round(clamp(Number(n) || 1, 1, max));
        els.countInput.max = String(max);
        if (!keepText) els.countInput.value = String(v);
        syncChips(v);
        return v;
    }

    function syncChips(v) {
        const chips = els.chips.children;
        for (let i = 0; i < chips.length; i++) {
            const chip = chips[i];
            const n = parseInt(chip.getAttribute('data-count'), 10);
            chip.classList.toggle('on', n === v);
        }
    }

    // ---------------- 事件绑定 ----------------

    els.chips.addEventListener('click', function (e) {
        const chip = e.target.closest ? e.target.closest('.chip') : null;
        if (!chip) return;
        setCount(parseInt(chip.getAttribute('data-count'), 10), false);
    });
    els.minus.addEventListener('click', function () { setCount(currentCount() - 1, false); });
    els.plus.addEventListener('click', function () { setCount(currentCount() + 1, false); });
    els.countInput.addEventListener('input', function () {
        const n = parseInt(els.countInput.value, 10);
        if (Number.isFinite(n)) syncChips(n);
    });
    els.countInput.addEventListener('change', function () { setCount(currentCount(), false); });
    els.countInput.addEventListener('blur', function () { setCount(currentCount(), false); });
    els.start.addEventListener('click', function () { startRound(setCount(currentCount(), false)); });
    els.action.addEventListener('click', onAction);
    els.skipPage.addEventListener('click', skipPage);
    els.restart.addEventListener('click', backToSetup);

    window.addEventListener('resize', function () {
        if (els.game.hidden) return;
        clearTimeout(resizeTimer);
        resizeTimer = setTimeout(function () {
            // 地址栏伸缩只会带来几像素变化，这种情况不重画，
            // 免得把用户已经刮开的进度洗掉
            for (let i = 0; i < state.pageSlots; i++) {
                const slot = slots[i];
                const rect = slot.canvas.getBoundingClientRect();
                const dw = Math.abs(rect.width - slot.cssW);
                const dh = Math.abs(rect.height - slot.cssH);
                if (dw < 8 && dh < 8) continue;
                slot.cssW = 0;
                slot.cssH = 0;
                measureSlots(state.pageSlots);
                break;
            }
        }, 200);
    });
    window.addEventListener('orientationchange', endAllStrokes);
    document.addEventListener('visibilitychange', function () {
        if (document.hidden) endAllStrokes();
    });
    window.addEventListener('blur', endAllStrokes);

    // ---------------- 启动 ----------------

    buildSlots();
    renderCounters();
    renderTotals();
    setCount(10, false);

    api.config().then(function (cfg) {
        if (cfg && typeof cfg === 'object') Object.assign(config, cfg);
        els.setupTip.innerHTML = '每页最多刮 <b>' + pageSize() + '</b> 次，刮完这页点“下一页”继续';
        setCount(currentCount(), false);
    }).catch(function () {
        els.footNote.textContent = '连不上服务器，请确认服务已经启动';
    });
})();
