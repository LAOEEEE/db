// Headless browser E2E for the scratch card via Chrome DevTools Protocol.
// Usage: node scripts/e2e-browser.mjs
// Requires Edge/Chrome; override with EDGE_BIN env var. Server must listen on 127.0.0.1:3100.
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import process from 'node:process';

const EDGE =
    process.env.EDGE_BIN ||
    'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe';
const PORT = 9333;
const URL = process.env.APP_URL || 'http://127.0.0.1:3100/';

const edge = spawn(
    EDGE,
    [
        '--headless=new',
        '--no-sandbox',
        '--disable-extensions',
        '--use-gl=angle',
        '--use-angle=swiftshader',
        '--enable-unsafe-swiftshader',
        `--remote-debugging-port=${PORT}`,
        '--user-data-dir=D:/ydd_workspace/db/.edge-e2e',
        '--window-size=440,860',
        'about:blank',
    ],
    { stdio: 'ignore' }
);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function cdpTarget() {
    for (let i = 0; i < 30; i++) {
        try {
            const res = await fetch(`http://127.0.0.1:${PORT}/json/list`);
            const list = await res.json();
            const page = list.find((t) => t.type === 'page');
            if (page) return page;
        } catch (_) {}
        await sleep(300);
    }
    throw new Error('CDP endpoint not available');
}

const target = await cdpTarget();
const ws = new WebSocket(target.webSocketDebuggerUrl);
let mid = 0;
const pending = new Map();
const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
        const id = ++mid;
        pending.set(id, { resolve, reject, method });
        ws.send(JSON.stringify({ id, method, params }));
    });
ws.onmessage = (ev) => {
    const msg = JSON.parse(ev.data);
    if (msg.id && pending.has(msg.id)) {
        const { resolve, reject, method } = pending.get(msg.id);
        pending.delete(msg.id);
        msg.error ? reject(new Error(method + ': ' + msg.error.message)) : resolve(msg.result);
    }
};
await new Promise((r) => (ws.onopen = r));
await send('Runtime.enable');
await send('Page.enable');

const evalJs = async (expression) => {
    const r = await send('Runtime.evaluate', {
        expression,
        returnByValue: true,
        awaitPromise: true,
    });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.text);
    return r.result.value;
};

const rectOf = async (sel) =>
    evalJs(`(() => {
        const r = document.querySelector(${JSON.stringify(sel)}).getBoundingClientRect();
        return {x: r.x, y: r.y, w: r.width, h: r.height};
    })()`);

const mouse = async (type, x, y, buttons) =>
    send('Input.dispatchMouseEvent', {
        type,
        x,
        y,
        button: type === 'mouseMoved' ? 'none' : 'left',
        buttons: buttons ?? (type === 'mousePressed' ? 1 : 0),
        clickCount: type === 'mouseMoved' ? 0 : 1,
    });

const touch = async (type, x, y) => {
    const params = { type, x, y };
    if (type !== 'touchEnd') params.touchPoints = [{ x, y, radiusX: 4, radiusY: 4, force: 0.8 }];
    else params.touchPoints = [];
    await send('Input.dispatchTouchEvent', params);
};

async function scratchArea(rect, pointer) {
    let y = rect.y + 12;
    let dir = true;
    await pointer('down', rect.x + 8, y);
    while (y < rect.y + rect.h - 10) {
        const xs = dir
            ? [rect.x + 8, rect.x + rect.w - 8]
            : [rect.x + rect.w - 8, rect.x + 8];
        const n = Math.ceil((xs[1] - xs[0]) / step) || 1;
        for (let i = 0; i <= Math.abs(n); i++) {
            const x = dir ? xs[0] + step * i : xs[0] - step * i;
            await pointer('move', x, y);
        }
        y += rowGap;
        dir = !dir;
        await sleep(12);
    }
    await pointer('up', rect.x + rect.w / 2, y);
}

const failures = [];
const check = (name, cond, extra) => {
    console.log(`${cond ? 'PASS' : 'FAIL'}  ${name}${extra !== undefined ? '  -> ' + JSON.stringify(extra) : ''}`);
    if (!cond) failures.push(name);
};

const step = 18;
const rowGap = 22;
const mousePointer = (kind, x, y) =>
    mouse(kind === 'down' ? 'mousePressed' : kind === 'up' ? 'mouseReleased' : 'mouseMoved', x, y, kind === 'move' ? 1 : undefined);
const touchPointer = (kind, x, y) =>
    touch(kind === 'down' ? 'touchStart' : kind === 'up' ? 'touchEnd' : 'touchMove', x, y);

try {
    await send('Page.navigate', { url: URL });
    await sleep(800);
    await send('Runtime.evaluate', { expression: 'localStorage.clear()' });
    await send('Page.reload', { ignoreCache: true });
    await sleep(2500);

    check('config loaded -> unlimited shown', /今日剩余：不限/.test(await evalJs(`document.getElementById('remaining').textContent`)));

    const btn = await rectOf('#action');
    await mouse('mousePressed', btn.x + btn.w / 2, btn.y + btn.h / 2);
    await mouse('mouseReleased', btn.x + btn.w / 2, btn.y + btn.h / 2);

    await sleep(900);
    const foilRect = await rectOf('#foil');
    check('foil visible after new card', foilRect.w > 200 && foilRect.h > 200, foilRect);
    check('hint hidden', await evalJs(`document.getElementById('hint').hidden`));
    check('button shows scratching state', /刮奖中/.test(await evalJs(`document.getElementById('action').textContent`)));

    // Gate: a tiny scratch inside one cell (<90% area, only 1 symbol) must NOT finish.
    await mouse('mousePressed', foilRect.x + 14, foilRect.y + 14);
    for (let d = 0; d <= 24; d += 6) {
        await mouse('mouseMoved', foilRect.x + 14 + d, foilRect.y + 14 + d, 1);
    }
    await mouse('mouseReleased', foilRect.x + 38, foilRect.y + 38);
    await sleep(700);
    const early = await evalJs(`JSON.stringify({result: document.getElementById('result').textContent, cleared: document.getElementById('foil').classList.contains('cleared')})`);
    const earlyState = JSON.parse(early);
    check('partial scratch does not finish early', !earlyState.cleared && earlyState.result === '', earlyState);

    // Zig-zag scratch covering well over the 90% threshold (and all 9 cells).
    await scratchArea(foilRect, mousePointer);
    await sleep(2500);

    const finalState = await evalJs(`(() => ({
        result: document.getElementById('result').textContent,
        resultClass: document.getElementById('result').className,
        cleared: document.getElementById('foil').classList.contains('cleared'),
        symbols: Array.from(document.querySelectorAll('.cell')).map(c => c.textContent),
        winning: Array.from(document.querySelectorAll('.cell.winning')).length,
        action: document.getElementById('action').textContent,
        actionDisabled: document.getElementById('action').disabled,
        remaining: document.getElementById('remaining').textContent,
        points: document.getElementById('points').textContent,
        cardWin: document.getElementById('card').classList.contains('win'),
    }))()`);

    check('foil faded out', finalState.cleared);
    check('all 9 cells filled', finalState.symbols.every((s) => s.length > 0), finalState.symbols);
    check('result message shown', finalState.result.length > 0, finalState.result);
    check('unlimited after card 1', /今日剩余：不限/.test(finalState.remaining), finalState.remaining);
    check('action ready for another card', /再来一张/.test(finalState.action) && !finalState.actionDisabled);

    if (/恭喜中奖/.test(finalState.result)) {
        check('win card gets gold cells + pulse', finalState.winning >= 3 && finalState.cardWin, finalState.winning);
        check('points awarded', !/积分：0$/.test(finalState.points), finalState.points);
    } else {
        check('losing card has no gold cells', finalState.winning === 0 && !finalState.cardWin);
        check('no points on loss', /积分：0$/.test(finalState.points), finalState.points);
    }

    // ---- second card in a row (mouse) ----
    const btn2 = await rectOf('#action');
    await mouse('mousePressed', btn2.x + btn2.w / 2, btn2.y + btn2.h / 2);
    await mouse('mouseReleased', btn2.x + btn2.w / 2, btn2.y + btn2.h / 2);
    await sleep(900);
    const foil2 = await rectOf('#foil');
    check('second card foil appears', foil2.w > 200, foil2);

    await scratchArea(foil2, mousePointer);
    await sleep(2500);

    const state2 = await evalJs(`(() => ({
        result: document.getElementById('result').textContent,
        remaining: document.getElementById('remaining').textContent,
        cleared: document.getElementById('foil').classList.contains('cleared'),
        filled: Array.from(document.querySelectorAll('.cell')).filter(c => c.textContent).length,
    }))()`);
    check('second card finished', state2.cleared && state2.filled === 9 && state2.result.length > 0, state2);
    check('unlimited after card 2', /今日剩余：不限/.test(state2.remaining), state2.remaining);

    // ---- third card via touch input (mobile path) ----
    await send('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 5 });
    const btn3 = await rectOf('#action');
    const tap = async (r) => {
        await touch('touchStart', r.x + r.w / 2, r.y + r.h / 2);
        await touch('touchEnd', r.x + r.w / 2, r.y + r.h / 2);
    };
    await tap(btn3);
    await sleep(900);
    const foil3 = await rectOf('#foil');
    check('third card foil appears (touch)', foil3.w > 200, foil3);
    await scratchArea(foil3, touchPointer);
    await sleep(2500);
    const state3 = await evalJs(`(() => ({
        result: document.getElementById('result').textContent,
        remaining: document.getElementById('remaining').textContent,
        cleared: document.getElementById('foil').classList.contains('cleared'),
        filled: Array.from(document.querySelectorAll('.cell')).filter(c => c.textContent).length,
    }))()`);
    check('touch card finished', state3.cleared && state3.filled === 9 && state3.result.length > 0, state3);
    check('unlimited after card 3', /今日剩余：不限/.test(state3.remaining), state3.remaining);

    const shot = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync('D:/ydd_workspace/db/screenshot-e2e.png', Buffer.from(shot.data, 'base64'));
    const shotJpg = await send('Page.captureScreenshot', { format: 'jpeg', quality: 85 });
    fs.writeFileSync('D:/ydd_workspace/db/screenshot-e2e.jpg', Buffer.from(shotJpg.data, 'base64'));
    console.log('screenshot -> screenshot-e2e.png/.jpg');
} finally {
    edge.kill();
    ws.close();
}

if (failures.length) {
    console.error(`\n${failures.length} check(s) failed`);
    process.exit(1);
}
console.log('\nALL CHECKS PASSED');
process.exit(0);
