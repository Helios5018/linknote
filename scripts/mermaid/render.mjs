import puppeteer from 'puppeteer';
import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

// No server or persistent browser: one isolated browser per render, using only
// installed JavaScript and system fonts. Notes cannot load external resources.
export async function render(source, { timeoutMs = 15000, onRequest, settings = {}, mode = "preview", viewport } = {}) {
  if (mode !== 'viewport' && (Buffer.byteLength(source) > 32768 || source.split('\n').length > 513)) {
    throw new Error('图表超过 32 KiB 或 512 行，请拆分图表');
  }
  let browser;
  let timer;
  try {
    browser = await puppeteer.launch({
      headless: true,
      timeout: timeoutMs,
      protocolTimeout: timeoutMs,
      args: ['--disable-background-networking', '--disable-component-update',
        '--disable-sync', '--no-first-run', '--no-default-browser-check',
        '--host-resolver-rules=MAP * ~NOTFOUND'],
    });
    const work = async () => {
      const page = await browser.newPage();
      page.setDefaultTimeout(timeoutMs);
      await page.setRequestInterception(true);
      page.on('request', request => {
        onRequest?.(request.url());
        void request.abort();
      });
      await page.setViewport({ width: 1600, height: 1200, deviceScaleFactor: 1 });
      await page.setContent(`<!doctype html><html><head>
        <meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src 'none'; connect-src 'none'">
        <style>body {margin:0;background:transparent} #diagram {display:inline-block;padding:8px}
        svg {font-family:Arial,'PingFang SC','Noto Sans CJK SC',sans-serif}</style>
        </head><body><div id="diagram"></div></body></html>`);
      if (mode === 'viewport') {
        const vector = JSON.parse(source);
        const { width, height, x, y, scale } = viewport;
        if (![width, height, x, y, scale].every(Number.isFinite) ||
            width < 1 || height < 1 || width > 4096 || height > 4096 ||
            width * height > 8000000 || scale <= 0) throw new Error('查看区域超出限制');
        await page.setViewport({ width, height, deviceScaleFactor: 1 });
        await page.evaluate(({ vector, x, y, scale }) => {
          document.body.style.background = vector.background;
          const container = document.getElementById('diagram');
          container.style.cssText = 'position:absolute;padding:0';
          container.style.left = `${x}px`;
          container.style.top = `${y}px`;
          container.innerHTML = vector.svg;
          const element = container.querySelector('svg');
          element.style.maxWidth = 'none';
          element.setAttribute('width', String(vector.width * scale));
          element.setAttribute('height', String(vector.height * scale));
          document.documentElement.style.overflow = 'hidden';
        }, { vector, x, y, scale });
        await page.evaluate(() => document.fonts.ready);
        return await page.screenshot({ type: 'png', omitBackground: true });
      }
      // ZenUML reads UI preferences from localStorage on import. The isolated
      // about:blank document has no origin/storage; keep those preferences in RAM.
      await page.evaluate(() => {
        const values = new Map();
        Object.defineProperty(window, 'localStorage', { value: {
          getItem: key => values.get(String(key)) ?? null,
          setItem: (key, value) => values.set(String(key), String(value)),
          removeItem: key => values.delete(String(key)),
          clear: () => values.clear(),
          key: index => [...values.keys()][index] ?? null,
          get length() { return values.size; },
        } });
      });
      await page.addScriptTag({ content: await readFile(new URL('bundle.js', import.meta.url), 'utf8') });
      const size = await page.evaluate(async ({ input, settings, mode }) => {
        const appDark = settings.theme !== 'light';
        const theme = !settings.mermaid_theme || settings.mermaid_theme === 'auto'
          ? (appDark ? 'dark' : 'default') : settings.mermaid_theme;
        const fontSize = `${settings.mermaid_font_size ?? 16}px`;
        window.mermaid.initialize({
          startOnLoad: false,
          securityLevel: 'strict',
          suppressErrorRendering: true,
          maxTextSize: 32768,
          maxEdges: 512,
          theme,
          themeVariables: { fontSize },
          fontFamily: "Arial, PingFang SC, Noto Sans CJK SC, sans-serif",
          // Prevent diagram directives/frontmatter from relaxing local-only policy.
          secure: ['secure', 'securityLevel', 'startOnLoad', 'maxTextSize', 'maxEdges',
            'suppressErrorRendering'],
        });
        const { svg } = await window.mermaid.render('mermaid-diagram', input);
        const effective = window.mermaid.mermaidAPI.getConfig();
        const canvas = settings.mermaid_canvas ?? 'auto';
        const background = canvas === 'transparent' ? 'transparent'
          : canvas === 'dark' || (canvas === 'auto' && effective.theme === 'dark') ? '#161b22'
          : '#ffffff';
        document.body.style.background = background;
        const container = document.getElementById('diagram');
        container.innerHTML = svg;
        const element = container.querySelector('svg');
        element.style.backgroundColor = background;
        const box = element.viewBox.baseVal;
        const natural = element.getBoundingClientRect();
        const width = box.width || natural.width;
        const height = box.height || natural.height;
        if (!(width > 0 && height > 0 && Number.isFinite(width + height))) {
          throw new Error('图表没有可显示的内容');
        }
        if (mode === 'vector') {
          element.style.maxWidth = 'none';
          element.setAttribute('width', String(width));
          element.setAttribute('height', String(height));
          return { svg: element.outerHTML, width, height, background };
        }
        const scale = Math.min(1584 / width, 2384 / height, 2);
        element.style.maxWidth = 'none';
        element.setAttribute('width', String(Math.floor(width * scale)));
        element.setAttribute('height', String(Math.floor(height * scale)));
        return { width: Math.ceil(width * scale) + 16, height: Math.ceil(height * scale) + 16 };
      }, { input: source, settings, mode });
      if (mode === 'vector') return Buffer.from(JSON.stringify(size));
      await page.setViewport({ width: size.width, height: size.height, deviceScaleFactor: 1 });
      await page.evaluate(() => document.fonts.ready);
      return await page.screenshot({ type: 'png', omitBackground: true });
    };
    return await Promise.race([
      work(),
      new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('图表渲染超时')), timeoutMs); }),
    ]);
  } finally {
    clearTimeout(timer);
    await browser?.close();
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    const source = await readFile(process.argv[2], 'utf8');
    const settings = process.argv[4] ? JSON.parse(process.argv[4]) : {};
    const options = process.argv[5] ? JSON.parse(process.argv[5]) : {};
    await writeFile(process.argv[3], await render(source, { settings, ...options }));
  } catch (error) {
    console.error(String(error.message).slice(0, 3000));
    process.exitCode = 1;
  }
}
