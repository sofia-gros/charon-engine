// Web サンドボックスの実ブラウザー・スモークテスト。
//
// 使い方:
//   scripts/build-web.sh
//   python3 -m http.server 8766 --directory apps/sandbox/web &
//   node tools/web-smoke/smoke.js [http://127.0.0.1:8766/index.html]
//
// Playwright と Chromium が必要 (リポジトリの依存ではない)。見つからないときは
// 環境変数 PLAYWRIGHT_PATH で playwright モジュールの場所を指定する。
//
// 確認すること (どれか 1 つでも崩れたら失敗):
//   - ページのエラー (panic を含む) が出ない
//   - 状態欄が「実行中」で、起動失敗の表示になっていない
//   - 描画ループが回り続けている (3 秒で 60 フレーム以上)
//   - キャンバスが既定の 300×150 のままでなく、ウィンドウに合わせて大きくなっている
//
// 過去にこのテストが捕まえた不具合:
//   - wasm で std::time::Instant が panic する (停止して `unreachable` になる)
//   - GPU の初期化が Resized より遅れると、サーフェスが未構成のまま再描画が止まる
//     (navigator.gpu があるのにアダプターがないブラウザーで再現した)
//
// ヘッドレス Chromium は WebGPU のキャンバスへ提示するとデバイスが破棄されるため、このテストは
// WebGL2 の経路だけを確かめる。WebGPU の描画は実ブラウザー (実 GPU) で別に確認する。

const { chromium } = require(process.env.PLAYWRIGHT_PATH || 'playwright');
const url = process.argv[2] || 'http://127.0.0.1:8766/index.html';

const configs = {
  // 既定のヘッドレス: navigator.gpu はあるがアダプターがない → WebGL2 へ落ちる。
  'navigator.gpu あり・アダプターなし': async () => {},
  // WebGPU に対応していないブラウザー: navigator.gpu がない。
  'navigator.gpu なし': async (page) => {
    await page.addInitScript(() => {
      Object.defineProperty(Navigator.prototype, 'gpu', { get() { return undefined; }, configurable: true });
    });
  },
};

(async () => {
  let failed = false;
  for (const [name, setup] of Object.entries(configs)) {
    const browser = await chromium.launch();
    const page = await (await browser.newContext({ viewport: { width: 640, height: 420 } })).newPage();
    const errors = [];
    page.on('pageerror', (e) => errors.push(e.message.slice(0, 200)));
    await page.addInitScript(() => {
      window.__frames = 0;
      const raf = window.requestAnimationFrame.bind(window);
      window.requestAnimationFrame = (f) => raf((t) => { window.__frames++; f(t); });
    });
    await setup(page);
    await page.goto(url);
    await page.waitForTimeout(8000);
    const before = await page.evaluate(() => window.__frames);
    await page.waitForTimeout(3000);
    const state = await page.evaluate(() => ({
      frames: window.__frames,
      status: document.getElementById('status').textContent,
      width: document.querySelector('canvas')?.width ?? 0,
    }));
    const delta = state.frames - before;
    const problems = [];
    if (errors.length) problems.push(`ページのエラー: ${errors.join(' | ')}`);
    if (!state.status.startsWith('実行中')) problems.push(`状態欄が「実行中」でない: ${state.status}`);
    if (delta < 60) problems.push(`描画ループが止まっている (3 秒で ${delta} フレーム)`);
    if (state.width <= 300) problems.push(`キャンバスが既定サイズのまま (幅 ${state.width})`);
    console.log(`${problems.length ? 'FAIL' : 'ok  '} ${name}: 3 秒で ${delta} フレーム、キャンバス幅 ${state.width}`);
    for (const p of problems) console.log(`       - ${p}`);
    failed ||= problems.length > 0;
    await browser.close();
  }
  process.exit(failed ? 1 : 0);
})();
