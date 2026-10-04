import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { render } from './render.mjs';

test('diagrams cannot request remote images or styles, even with a loose directive', async () => {
  let hits = 0;
  const server = createServer((_, response) => { hits++; response.end('unexpected'); });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const url = `http://127.0.0.1:${server.address().port}/tracking`;
  try {
    const source = `%%{init: {"securityLevel":"loose","themeCSS":"@import url('${url}');"}}%%
flowchart LR
    A["<img src='${url}'>Local"] --> B[Safe]`;
    const png = await render(source);
    assert.deepEqual(Buffer.from(png.slice(0, 8)), Buffer.from([137,80,78,71,13,10,26,10]));
    assert.equal(hits, 0);
  } finally {
    await new Promise(resolve => server.close(resolve));
  }
});

test('syntax errors are reported without a PNG or hanging browser', async () => {
  await assert.rejects(render('flowchart LR\n A[unclosed'), /Parse error/);
});

test('vector viewports retain text, crop at zoom, bound output, and block remote resources', async () => {
  let hits = 0;
  const server = createServer((_, response) => { hits++; response.end('unexpected'); });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const url = `http://127.0.0.1:${server.address().port}/tracking`;
  try {
    const vector = JSON.parse(await render('flowchart LR\n A[中文细节] --> B[Zoom]', { mode: 'vector' }));
    assert.ok(vector.svg.includes('中文细节'));
    assert.ok(vector.width > 0 && vector.height > 0);
    // Exercise the rasterizer's independent network isolation too.
    vector.svg = vector.svg.replace('</svg>', `<image href="${url}" width="20" height="20"/></svg>`);
    const source = JSON.stringify(vector);
    const viewport = { width: 800, height: 400, x: 0, y: 0, scale: 1 };
    const first = Buffer.from(await render(source, { mode: 'viewport', viewport }));
    const zoomed = Buffer.from(await render(source, { mode: 'viewport', viewport: { ...viewport, scale: 4, x: -100, y: -100 } }));
    assert.equal(first.readUInt32BE(16), 800);
    assert.equal(first.readUInt32BE(20), 400);
    assert.equal(zoomed.readUInt32BE(16), 800);
    assert.equal(zoomed.readUInt32BE(20), 400);
    assert.notDeepEqual(first, zoomed);
    assert.equal(hits, 0);
    await assert.rejects(render(source, { mode: 'viewport', viewport: { ...viewport, width: 100000 } }), /超出限制/);
  } finally {
    await new Promise(resolve => server.close(resolve));
  }
});
