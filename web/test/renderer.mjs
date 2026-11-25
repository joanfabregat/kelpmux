import { chromium } from "playwright";
import { stripTypeScriptTypes } from "node:module";
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";
const source = stripTypeScriptTypes(
  readFileSync(
    new URL(
      "../../web-core/src/renderer.ts",
      import.meta.url,
    ),
    "utf8",
  ),
  {
    mode: "transform",
  },
);
const browser = await chromium.launch({ headless: true });
try {
  for (const ratio of [1, 2, 3]) {
    const context = await browser.newContext({
      deviceScaleFactor: ratio,
      viewport: { width: 900, height: 600 },
    });
    const page = await context.newPage();
    await page.setContent(
      '<canvas style="width:800px;height:400px;--background:#101113"></canvas>',
    );
    const result = await page.evaluate(
      async ({ source, ratio }) => {
        const moduleURL = URL.createObjectURL(
          new Blob([source], { type: "text/javascript" }),
        );
        const { Renderer } = await import(moduleURL);
        URL.revokeObjectURL(moduleURL);
        const renderer = new Renderer(document.querySelector("canvas"));
        if (!renderer.gl)
          throw Error("WebGL2 unavailable in renderer regression");
        // Force several atlas pages in one frame, including styled glyphs.
        renderer.atlas.width = 256;
        renderer.atlas.height = 256;
        const cols = 70,
          rows = 14;
        const blank = {
          text: " ",
          width: 1,
          fg: "Default",
          bg: "Default",
          bold: false,
          italic: false,
          underline: false,
        };
        const cells = Array.from({ length: cols * rows }, () => ({ ...blank }));
        for (let i = 0; i < cols * 2; i++)
          cells[i] = {
            ...blank,
            text: String.fromCharCode(33 + (i % 90)),
            italic: i % 2 === 0,
            bold: i % 3 === 0,
          };
        renderer.draw({ cols, rows, cells, modes: {} });
        const gl = renderer.gl;
        const pixels = new Uint8Array(800 * ratio * 100 * ratio * 4);
        // A blank band well below the two text rows must remain entirely blank.
        gl.readPixels(
          0,
          150 * ratio,
          800 * ratio,
          100 * ratio,
          gl.RGBA,
          gl.UNSIGNED_BYTE,
          pixels,
        );
        let corrupted = 0;
        for (let i = 0; i < pixels.length; i += 4)
          if (
            Math.abs(pixels[i] - 16) > 1 ||
            Math.abs(pixels[i + 1] - 17) > 1 ||
            Math.abs(pixels[i + 2] - 19) > 1
          )
            corrupted++;
        const image = new Uint8Array(
          renderer.canvas.width * renderer.canvas.height * 4,
        );
        gl.readPixels(
          0,
          0,
          renderer.canvas.width,
          renderer.canvas.height,
          gl.RGBA,
          gl.UNSIGNED_BYTE,
          image,
        );
        const referenceCanvas = renderer.canvas.cloneNode(false);
        document.body.append(referenceCanvas);
        const reference = new Renderer(referenceCanvas);
        reference.draw({ cols, rows, cells, modes: {} });
        const expected = new Uint8Array(image.length);
        reference.gl.readPixels(
          0,
          0,
          reference.canvas.width,
          reference.canvas.height,
          gl.RGBA,
          gl.UNSIGNED_BYTE,
          expected,
        );
        let differing = 0;
        for (let i = 0; i < image.length; i++)
          if (Math.abs(image[i] - expected[i]) > 1) differing++;
        reference.dispose();
        const cursorScreen = {
          cols,
          rows,
          cells: cells.map(() => ({ ...blank })),
          modes: {},
          cursor: { visible: true, position: { x: 0, y: 0 } },
        };
        renderer.canvas.style.setProperty("--terminal-cursor", "#ffff00");
        renderer.draw(cursorScreen);
        const cursorMargin = new Uint8Array(4),
          cursorInside = new Uint8Array(4);
        gl.readPixels(
          1,
          renderer.canvas.height - 1,
          1,
          1,
          gl.RGBA,
          gl.UNSIGNED_BYTE,
          cursorMargin,
        );
        gl.readPixels(
          1,
          renderer.canvas.height - (renderer.cursorTop + 1) * ratio,
          1,
          1,
          gl.RGBA,
          gl.UNSIGNED_BYTE,
          cursorInside,
        );
        const fallbackCanvas = renderer.canvas.cloneNode(false);
        document.body.append(fallbackCanvas);
        const getContext = fallbackCanvas.getContext.bind(fallbackCanvas);
        fallbackCanvas.getContext = (type, ...options) =>
          type === "webgl2" ? null : getContext(type, ...options);
        const fallback = new Renderer(fallbackCanvas);
        fallback.draw(cursorScreen);
        const fallbackMargin = Array.from(
          fallback.ctx.getImageData(1, 0, 1, 1).data,
        );
        const fallbackInside = Array.from(
          fallback.ctx.getImageData(1, (fallback.cursorTop + 1) * ratio, 1, 1)
            .data,
        );
        fallbackCanvas.style.width = "250px";
        fallback.draw(cursorScreen);
        const fallbackScale = fallback.ctx.getTransform().a;
        function checkHistory(target) {
          target.historyLayout = (_lines, cols, rows) => ({
            cols,
            rows,
            modes: {},
            cells: Array.from({ length: cols * rows }, () => ({ ...blank })),
          });
          target.historyMatch = { start: { x: 3, y: 2 }, end: { x: 5, y: 2 } };
          const lines = ["history"];
          const pixel = (column) => {
            const x = Math.floor((column * target.cw + 1) * target.dpr);
            const y = Math.floor((2 * target.ch + 1) * target.dpr);
            if (target.gl) {
              const p = new Uint8Array(4);
              target.gl.readPixels(
                x,
                target.canvas.height - y - 1,
                1,
                1,
                target.gl.RGBA,
                target.gl.UNSIGNED_BYTE,
                p,
              );
              return Array.from(p);
            }
            return Array.from(target.ctx.getImageData(x, y, 1, 1).data);
          };
          target.drawHistory(lines);
          const highlighted = pixel(3),
            outside = pixel(2);
          target.historyMatch = null;
          target.drawHistory(lines);
          return { highlighted, outside, cleared: pixel(3) };
        }
        const gpuHistory = checkHistory(renderer),
          cpuHistory = checkHistory(fallback);
        fallback.dispose();
        const result = {
          gpuHistory,
          cpuHistory,
          cursorMargin: Array.from(cursorMargin),
          cursorInside: Array.from(cursorInside),
          fallbackMargin,
          fallbackInside,
          dpr: renderer.dpr,
          font: renderer.atlasCtx.font,
          width: renderer.canvas.width,
          height: renderer.canvas.height,
          tileHeight: renderer.tileHeight(),
          corrupted,
          differing,
          error: gl.getError(),
        };
        let drawnCell = 0;
        const bufferData = gl.bufferData.bind(gl);
        gl.bufferData = (target, data, usage) => {
          if (data instanceof Float32Array && data.length) drawnCell = data[2];
          return bufferData(target, data, usage);
        };
        renderer.canvas.style.width = "250px";
        renderer.draw(cursorScreen);
        const rect = renderer.canvas.getBoundingClientRect();
        result.point = renderer.point(
          {
            clientX: rect.left + renderer.cw * 3 + 0.5,
            clientY: rect.top + renderer.ch * 2 + 0.5,
          },
          cursorScreen,
        );
        result.naturalWidth = drawnCell;
        result.expectedWidth = renderer.cw * ratio;
        result.fallbackScale = fallbackScale;
        renderer.canvas.style.setProperty("--background", "#132738");
        renderer.clear();
        const clearPixel = new Uint8Array(4);
        gl.readPixels(1, 1, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, clearPixel);
        result.clearPixel = Array.from(clearPixel);
        renderer.dispose();
        return result;
      },
      { source, ratio },
    );
    assert.equal(result.dpr, ratio);
    assert.match(result.font, new RegExp(`${14 * ratio}px`));
    assert.equal(result.width, 800 * ratio);
    assert.equal(result.height, 400 * ratio);
    assert.equal(result.tileHeight, 20 * ratio);
    assert.equal(result.corrupted, 0, `blank-cell glyph leakage at ${ratio}x`);
    assert.equal(
      result.differing,
      0,
      `atlas rollover must preserve every glyph at ${ratio}x`,
    );
    assert.equal(result.error, 0);
    for (const history of [result.gpuHistory, result.cpuHistory]) {
      assert.deepEqual(history.highlighted, [110, 84, 20, 255]);
      assert.deepEqual(history.outside, [16, 17, 19, 255]);
      assert.deepEqual(history.cleared, history.outside);
    }
    assert.equal(
      result.naturalWidth,
      result.expectedWidth,
      "pending resize must clip rather than squash glyphs",
    );
    assert.equal(result.fallbackScale, ratio);
    assert.deepEqual(result.point, { x: 3, y: 2 });
    assert.deepEqual(
      result.clearPixel,
      [19, 39, 56, 255],
      "empty canvases must use the terminal background",
    );
    assert.deepEqual(result.cursorMargin, [16, 17, 19, 255]);
    assert.deepEqual(result.cursorInside, [255, 255, 0, 255]);
    assert.deepEqual(result.fallbackMargin, result.cursorMargin);
    assert.deepEqual(result.fallbackInside, result.cursorInside);
    console.log(
      `PASS: ${ratio}x glyph rasterization, physical canvas size, atlas recycling and blank cells`,
    );
    await context.close();
  }
  for (const ratio of [1, 1.25, 2, 3]) {
    const context = await browser.newContext({ deviceScaleFactor: ratio });
    const page = await context.newPage();
    const result = await page.evaluate(
      async ({ source }) => {
        const moduleURL = URL.createObjectURL(
          new Blob([source], { type: "text/javascript" }),
        );
        const { Renderer } = await import(moduleURL);
        URL.revokeObjectURL(moduleURL);
        const results = [];
        for (const fallback of [false, true]) {
          const canvas = document.createElement("canvas");
          canvas.style.cssText =
            "width:600px;height:200px;--background:#101113;--terminal-font:monospace";
          document.body.append(canvas);
          if (fallback) {
            const get = canvas.getContext.bind(canvas);
            canvas.getContext = (kind, ...args) =>
              kind === "webgl2" ? null : get(kind, ...args);
          }
          const renderer = new Renderer(canvas),
            cols = 16;
          const cells = Array.from({ length: cols * 3 }, (_, i) => ({
            text: ["█", "▀", "▄"][Math.floor(i / cols)],
            width: 1,
            fg: { Rgb: [220, 100, 60] },
            bg: "Default",
            bold: false,
            italic: false,
            underline: false,
          }));
          renderer.draw({ cols, rows: 3, cells, modes: {} });
          const w = Math.round(cols * renderer.cw * renderer.dpr),
            h = Math.round(renderer.ch * renderer.dpr);
          let gaps = 0;
          for (const y of [1, h - 2, h + 1, 3 * h - 2]) {
            let pixels;
            if (renderer.gl) {
              pixels = new Uint8Array(w * 4);
              renderer.gl.readPixels(
                0,
                canvas.height - y - 1,
                w,
                1,
                renderer.gl.RGBA,
                renderer.gl.UNSIGNED_BYTE,
                pixels,
              );
            } else pixels = renderer.ctx.getImageData(0, y, w, 1).data;
            for (let x = 0; x < w; x++)
              if (
                pixels[x * 4] !== 220 ||
                pixels[x * 4 + 1] !== 100 ||
                pixels[x * 4 + 2] !== 60
              )
                gaps++;
          }
          const ctx = renderer.atlasCtx;
          ctx.font = `14px ${renderer.family}`;
          results.push({
            fallback,
            gaps,
            advanceError:
              Math.abs(renderer.cw - ctx.measureText("M").width) * renderer.dpr,
          });
          renderer.dispose();
          canvas.remove();
        }
        return results;
      },
      { source },
    );
    for (const item of result) {
      assert.equal(
        item.gaps,
        0,
        `${ratio}x ${item.fallback ? "Canvas" : "WebGL"} terminal blocks must join without seams`,
      );
      assert(
        item.advanceError <= 0.501,
        "font advance must stay within half a physical pixel",
      );
    }
    console.log(
      `PASS: ${ratio}x seamless full/half blocks in WebGL and Canvas, device-pixel font advance`,
    );
    await context.close();
  }
} finally {
  await browser.close();
}
