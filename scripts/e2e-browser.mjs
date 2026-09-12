// Headless browser E2E for the scratch card via Chrome DevTools Protocol.
// Usage: node scripts/e2e-browser.mjs
// Requires Edge/Chrome; override with EDGE_BIN env var. Server must listen on 127.0.0.1:3100.
//
// 覆盖的新玩法：
//   设置次数 -> 一页 5 格逐格刮开 -> 未刮/已刮计数 -> 中奖格变黄 -> 翻到下一页 -> 刮完
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import process from 'node:process';

const EDGE =
    process.env.EDGE_BIN ||
    'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe';
const PORT = 9333;
const URL = process.env.APP_URL || 'http://127.0.0.1:3100/';
const PAGE_SIZE = 5;
const TOTAL = 10;              // 设成 10 次 -> 正好两页，用来验证翻页
const scratchStep = 16;
const rowGap = 12;

const edge = spawn(
    EDGE,
    [
        '--headless=new',
        '--no-sandbox',
        '--disable-extensions',
        '--use-gl=angle',
        '--use-angle=swiftshader',
        '--enable-unsafe-swiftshader',
        '--remote-debugging-port=' + PORT,
        '--user-data-dir=D:/ydd_workspace/db/.edge-e2e',
        '--window-size=440,900',
        'about:blank',
    ],
    { stdio: 'ignore' }
);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function cdpTarget() {
    for (let i = 0; i < 30; i++) {
        try {
            const res = await fetch('http://127.0.0.1:' + PORT + '/json/list');
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
    evalJs('(() => {' +
        'const el = document.querySelector(' + JSON.stringify(sel) + ');' +
        'if (!el) return null;' +
        'const r = el.getBoundingClientRect();' +
        'return {x: r.x, y: r.y, w: r.width, h: r.height};' +
        '})()');

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

const mousePointer = (kind, x, y) =>
    mouse(kind === 'down' ? 'mousePressed' : kind === 'up' ? 'mouseReleased' : 'mouseMoved',
        x, y, kind === 'move' ? 1 : undefined);
const touchPointer = (kind, x, y) =>
    touch(kind === 'down' ? 'touchStart' : kind === 'up' ? 'touchEnd' : 'touchMove', x, y);

async function clickSelector(sel) {
    const r = await rectOf(sel);
    if (!r) throw new Error('element not found: ' + sel);
    await mousePointer('down', r.x + r.w / 2, r.y + r.h / 2);
    await mousePointer('up', r.x + r.w / 2, r.y + r.h / 2);
}

// 在单个刮奖格上来回刮满（格子矮，5 个来回就远超阈值）
async function scratchSlot(rect, pointer) {
    let y = rect.y + 6;
    let dir = true;
    await pointer('down', rect.x + 6, y);
    while (y < rect.y + rect.h - 4) {
        const xs = dir ? [rect.x + 6, rect.x + rect.w - 6] : [rect.x + rect.w - 6, rect.x + 6];
        const n = Math.max(1, Math.ceil(Math.abs(xs[1] - xs[0]) / scratchStep));
        for (let i = 0; i <= n; i++) {
            const x = dir ? xs[0] + scratchStep * i : xs[0] - scratchStep * i;
            await pointer('move', x, y);
        }
        y += rowGap;
        dir = !dir;
        await sleep(10);
    }
    await pointer('up', rect.x + rect.w / 2, y);
}

const pageState = () =>
    evalJs('(() => {' +
        'const q = (s) => document.querySelector(s);' +
        'const slots = Array.from(document.querySelectorAll(".slot:not([hidden])"));' +
        'const amountEls = slots.map((s) => s.querySelector(".amount"));' +
        'const winFace = q(".slot.win .face");' +
        'return {' +
        '  left: q("#leftNum").textContent,' +
        '  done: q("#doneNum").textContent,' +
        '  page: q("#pageNum").textContent,' +
        '  amounts: amountEls.map((a) => a.textContent),' +
        '  emptyAmounts: amountEls.filter((a) => !a.textContent).length,' +
        '  wins: slots.filter((s) => s.classList.contains("win")).length,' +
        '  winBg: winFace ? getComputedStyle(winFace).backgroundImage : "",' +
        '  result: q("#result").textContent,' +
        '  action: q("#action").textContent,' +
        '  actionDisabled: q("#action").disabled,' +
        '};' +
        '})()');

const failures = [];
const check = (name, cond, extra) => {
    console.log((cond ? 'PASS' : 'FAIL') + '  ' + name +
        (extra !== undefined ? '  -> ' + JSON.stringify(extra) : ''));
    if (!cond) failures.push(name);
};

try {
    await send('Page.navigate', { url: URL });
    await sleep(800);
    await send('Runtime.evaluate', { expression: 'localStorage.clear()' });
    await send('Page.reload', { ignoreCache: true });
    await sleep(2500);

    // ---- 设置页：用户自己填次数 ----
    check('setup screen visible', await evalJs('!document.getElementById("setup").hidden'));
    check('default count is 10', (await evalJs('document.getElementById("countInput").value')) === '10');

    await evalJs('(() => {' +
        'const i = document.getElementById("countInput");' +
        'i.value = "' + TOTAL + '";' +
        'i.dispatchEvent(new Event("change", { bubbles: true }));' +
        'return i.value;' +
        '})()');
    await clickSelector('#start');
    await sleep(1800);

    check('game screen shown', await evalJs('!document.getElementById("game").hidden'));

    const slotsShown = await evalJs(
        'Array.from(document.querySelectorAll(".slot")).filter((s) => !s.hidden).length');
    check('page 1 shows 5 slots', slotsShown === PAGE_SIZE, slotsShown);

    let s = await pageState();
    check('counters: unscratch=10 done=0', s.left === '10' && s.done === '0', s);
    check('page counter is 1/2', s.page === '1/2', s.page);
    check('all slots still covered', s.emptyAmounts === PAGE_SIZE, s);

    // ---- 轻轻刮一下，确认不会提前开奖 ----
    const foil1 = await rectOf('.slot:nth-child(1) .foil');
    await mousePointer('down', foil1.x + 10, foil1.y + 10);
    await mousePointer('move', foil1.x + 40, foil1.y + 10);
    await mousePointer('up', foil1.x + 40, foil1.y + 10);
    await sleep(500);
    s = await pageState();
    check('partial scratch does not reveal early', s.done === '0' && s.emptyAmounts === PAGE_SIZE, s);

    // ---- 逐格刮开整页 ----
    for (let i = 1; i <= PAGE_SIZE; i++) {
        const rect = await rectOf('.slot:nth-child(' + i + ') .foil');
        await scratchSlot(rect, mousePointer);
        await sleep(700);
    }
    await sleep(1500);

    s = await pageState();
    check('page 1 fully scratched', s.done === '5' && s.left === '5', s);
    check('every slot shows a money amount',
        s.emptyAmounts === 0 && s.amounts.every((a) => /^\$\d+$/.test(a)), s.amounts);
    check('page 1 settled automatically', s.result.length > 0, s.result);
    check('action ready for next page', /进入下一页/.test(s.action) && !s.actionDisabled, s.action);
    if (s.wins > 0) {
        check('winning slot has yellow background', /gradient/.test(s.winBg), s.winBg);
    } else {
        check('no win on page 1 -> no yellow slot', s.wins === 0);
    }

    // ---- 翻到第二页 ----
    await clickSelector('#action');
    await sleep(1800);

    s = await pageState();
    check('page 2 shown', s.page === '2/2' && s.done === '5' && s.left === '5', s);
    check('page 2 has 5 fresh slots', s.emptyAmounts === PAGE_SIZE, s);

    for (let i = 1; i <= PAGE_SIZE; i++) {
        const rect = await rectOf('.slot:nth-child(' + i + ') .foil');
        await scratchSlot(rect, mousePointer);
        await sleep(700);
    }
    await sleep(1500);

    s = await pageState();
    check('round finished: unscratch=0 done=10', s.left === '0' && s.done === '10', s);
    check('action offers restart', /重新设置/.test(s.action) && !s.actionDisabled, s.action);
    check('round summary shown', /本轮结束/.test(s.result), s.result);

    // ---- 触摸路径：重新开一轮，用触摸刮开第一格 ----
    await send('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 5 });
    await clickSelector('#action');           // 重新设置次数
    await sleep(600);
    await clickSelector('#start');
    await sleep(1800);
    const foilTouch = await rectOf('.slot:nth-child(1) .foil');
    check('touch round started', !!foilTouch && foilTouch.w > 100, foilTouch);
    await scratchSlot(foilTouch, touchPointer);
    await sleep(900);
    s = await pageState();
    check('touch scratch reveals a slot', s.done === '1' && s.emptyAmounts === PAGE_SIZE - 1, s);

    const shotDir = 'D:/ydd_workspace/db';
    const shot = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(shotDir + '/screenshot-e2e.png', Buffer.from(shot.data, 'base64'));
    const shotJpg = await send('Page.captureScreenshot', { format: 'jpeg', quality: 85 });
    fs.writeFileSync(shotDir + '/screenshot-e2e.jpg', Buffer.from(shotJpg.data, 'base64'));
    console.log('screenshot -> ' + shotDir + '/screenshot-e2e.png/.jpg');
} finally {
    edge.kill();
    ws.close();
}

if (failures.length) {
    console.error('\n' + failures.length + ' check(s) failed');
    process.exit(1);
}
console.log('\nALL CHECKS PASSED');
process.exit(0);
