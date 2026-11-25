import type { Cell, Color, Screen } from "./types";
const ansi = [
  "#171a20",
  "#d46f77",
  "#88bd92",
  "#d9bd80",
  "#79ade0",
  "#b299cf",
  "#7fbdc1",
  "#d9dce2",
  "#646d7a",
  "#ed8793",
  "#a6da95",
  "#eed49f",
  "#8aadf4",
  "#c6a0f6",
  "#8bd5ca",
  "#ffffff",
];
export function color(
  value: Color | undefined,
  fallback: string,
  palette = ansi,
): string {
  if (!value || value === "Default") return fallback;
  if ("Rgb" in value) return `rgb(${value.Rgb.join(",")})`;
  const index = value.Indexed;
  if (index < 16) return palette[index];
  if (index >= 232) {
    const c = 8 + (index - 232) * 10;
    return `rgb(${c},${c},${c})`;
  }
  const n = index - 16;
  const v = [0, 95, 135, 175, 215, 255];
  return `rgb(${v[Math.floor(n / 36)]},${v[Math.floor(n / 6) % 6]},${v[n % 6]})`;
}
function rgba(css: string): number[] {
  if (css.startsWith("#"))
    return [
      parseInt(css.slice(1, 3), 16) / 255,
      parseInt(css.slice(3, 5), 16) / 255,
      parseInt(css.slice(5, 7), 16) / 255,
      1,
    ];
  return [
    ...css
      .match(/\d+/g)!
      .map(Number)
      .map((n) => n / 255),
    1,
  ];
}
export type Selection = {
  start: { x: number; y: number };
  end: { x: number; y: number };
} | null;
// Terminal block graphics must reach cell edges, independent of font bearings.
function paintBlock(
  ctx: CanvasRenderingContext2D,
  text: string,
  x: number,
  y: number,
  width: number,
  height: number,
): boolean {
  if (text.length !== 1) return false;
  const code = text.charCodeAt(0);
  if (code < 0x2580 || code > 0x259f) return false;
  const transform = ctx.getTransform();
  const sx = transform.a || 1,
    sy = transform.d || 1;
  const snapX = (value: number) => Math.round(value * sx) / sx;
  const snapY = (value: number) => Math.round(value * sy) / sy;
  const rect = (left: number, top: number, right: number, bottom: number) => {
    const x1 = snapX(x + left * width),
      y1 = snapY(y + top * height);
    ctx.fillRect(
      x1,
      y1,
      snapX(x + right * width) - x1,
      snapY(y + bottom * height) - y1,
    );
  };
  if (code === 0x2580) rect(0, 0, 1, 0.5);
  else if (code >= 0x2581 && code <= 0x2588)
    rect(0, 1 - (code - 0x2580) / 8, 1, 1);
  else if (code >= 0x2589 && code <= 0x258f) rect(0, 0, (0x2590 - code) / 8, 1);
  else if (code === 0x2590) rect(0.5, 0, 1, 1);
  else if (code === 0x2594) rect(0, 0, 1, 0.125);
  else if (code === 0x2595) rect(0.875, 0, 1, 1);
  else if (code >= 0x2596 && code <= 0x259f) {
    const masks = [4, 8, 1, 13, 9, 7, 11, 2, 6, 14];
    const mask = masks[code - 0x2596];
    if (mask & 1) rect(0, 0, 0.5, 0.5);
    if (mask & 2) rect(0.5, 0, 1, 0.5);
    if (mask & 4) rect(0, 0.5, 0.5, 1);
    if (mask & 8) rect(0.5, 0.5, 1, 1);
  } else return false;
  return true;
}
export class Renderer {
  historyLayout?: (lines: string[], cols: number, rows: number) => Screen;
  historyMatch: Selection = null;
  private historyCache?: {
    lines: string[];
    cols: number;
    rows: number;
    match: Selection;
    screen: Screen;
  };
  gl: WebGL2RenderingContext | null;
  ctx: CanvasRenderingContext2D | null = null;
  program?: WebGLProgram;
  buffer?: WebGLBuffer;
  atlas: HTMLCanvasElement;
  atlasCtx: CanvasRenderingContext2D;
  texture?: WebGLTexture;
  glyphs = new Map<string, number>();
  cw = 9;
  ch = 20;
  fontSize = 14;
  baseline = 15;
  cursorTop = 2;
  cursorHeight = 16;
  family = "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace";
  background = "#101113";
  foreground = "#d7dce3";
  palette = ansi;
  selectionBackground = "#385776";
  selectionForeground = "";
  cursor = "#d7dce3";
  cursorText = "#101113";
  dpr = 1;
  lost = false;
  atlasDirty = true;
  hoverLink: string | undefined;
  constructor(public canvas: HTMLCanvasElement) {
    this.atlas = document.createElement("canvas");
    this.atlas.width = 2048;
    this.atlas.height = 2048;
    this.atlasCtx = this.atlas.getContext("2d")!;
    this.gl = canvas.getContext("webgl2", {
      alpha: false,
      antialias: false,
      preserveDrawingBuffer: false,
    });
    if (this.gl) this.setup();
    else this.ctx = canvas.getContext("2d", { alpha: false });
    canvas.addEventListener("webglcontextlost", (e) => {
      e.preventDefault();
      this.lost = true;
    });
    canvas.addEventListener("webglcontextrestored", () => {
      this.lost = false;
      this.glyphs.clear();
      this.setup();
    });
    this.metrics();
  }
  metrics() {
    const ctx = this.atlasCtx;
    ctx.font = `${this.fontSize}px ${this.family}`;
    ctx.textBaseline = "alphabetic";
    const measured = ctx.measureText("Mg");
    this.cw = Math.max(
      1,
      Math.round(ctx.measureText("M").width * this.dpr) / this.dpr,
    );
    this.ch = Math.ceil(this.fontSize * 1.4);
    const ascent = Math.ceil(
      measured.actualBoundingBoxAscent || this.fontSize * 0.8,
    );
    const descent = Math.ceil(
      measured.actualBoundingBoxDescent || this.fontSize * 0.2,
    );
    this.baseline = Math.floor((this.ch - ascent - descent) / 2) + ascent;
    this.cursorTop = Math.max(0, this.baseline - ascent - 1);
    this.cursorHeight = Math.min(
      this.ch - this.cursorTop,
      ascent + descent + 2,
    );
  }
  appearance() {
    const style = getComputedStyle(this.canvas);
    const size = parseInt(style.getPropertyValue("--terminal-size")) || 14;
    const family =
      style.getPropertyValue("--terminal-font").trim() || this.family;
    this.background =
      style.getPropertyValue("--background").trim() || "#101113";
    this.foreground =
      style.getPropertyValue("--terminal-foreground").trim() || "#d7dce3";
    this.palette = ansi.map(
      (fallback, index) =>
        style.getPropertyValue(`--terminal-ansi-${index}`).trim() || fallback,
    );
    this.selectionBackground =
      style.getPropertyValue("--terminal-selection").trim() || "#385776";
    this.selectionForeground = style
      .getPropertyValue("--terminal-selection-text")
      .trim();
    this.cursor =
      style.getPropertyValue("--terminal-cursor").trim() || this.foreground;
    this.cursorText =
      style.getPropertyValue("--terminal-cursor-text").trim() ||
      this.background;
    const dpr = window.devicePixelRatio || 1;
    if (size !== this.fontSize || family !== this.family || dpr !== this.dpr) {
      this.fontSize = size;
      this.family = family;
      this.dpr = dpr;
      this.glyphs.clear();
      this.atlasCtx.clearRect(0, 0, 2048, 2048);
      this.atlasDirty = true;
      this.metrics();
    }
  }
  setup() {
    const gl = this.gl!;
    const vertex = `#version 300 es
      precision highp float;layout(location=0)in vec4 rect;layout(location=1)in vec4 texRect;layout(location=2)in vec4 foreground;layout(location=3)in vec4 background;
      uniform vec2 viewport;out vec2 uv;flat out vec4 glyphBounds;out vec4 fg;out vec4 bg;
      void main(){vec2 corners[6]=vec2[6](vec2(0,0),vec2(1,0),vec2(0,1),vec2(0,1),vec2(1,0),vec2(1,1));vec2 c=corners[gl_VertexID];vec2 p=rect.xy+c*rect.zw;
      gl_Position=vec4(p.x/viewport.x*2.0-1.0,1.0-p.y/viewport.y*2.0,0,1);uv=texRect.xy+c*texRect.zw;glyphBounds=vec4(texRect.xy,texRect.xy+texRect.zw);fg=foreground;bg=background;}`;
    const fragment = `#version 300 es
      precision highp float;uniform sampler2D atlas;uniform vec4 cursorRect;uniform vec4 cursorBg;uniform vec4 cursorFg;in vec2 uv;flat in vec4 glyphBounds;in vec4 fg;in vec4 bg;out vec4 result;
      void main(){vec2 inset=0.5/vec2(textureSize(atlas,0));float a=all(equal(fg,bg))?0.0:texture(atlas,clamp(uv,glyphBounds.xy+inset,glyphBounds.zw-inset)).a;bool cursor=all(greaterThanEqual(gl_FragCoord.xy,cursorRect.xy))&&all(lessThan(gl_FragCoord.xy,cursorRect.xy+cursorRect.zw));result=cursor?mix(cursorBg,cursorFg,a):mix(bg,fg,a);}`;
    const compile = (kind: number, source: string) => {
      const shader = gl.createShader(kind)!;
      gl.shaderSource(shader, source);
      gl.compileShader(shader);
      if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS))
        throw Error(gl.getShaderInfoLog(shader) || "Shader compilation failed");
      return shader;
    };
    const program = gl.createProgram()!;
    gl.attachShader(program, compile(gl.VERTEX_SHADER, vertex));
    gl.attachShader(program, compile(gl.FRAGMENT_SHADER, fragment));
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS))
      throw Error("Terminal renderer could not initialize");
    this.program = program;
    this.buffer = gl.createBuffer()!;
    this.texture = gl.createTexture()!;
    this.atlasDirty = true;
    gl.bindTexture(gl.TEXTURE_2D, this.texture);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.useProgram(program);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.buffer);
    for (let i = 0; i < 4; i++) {
      gl.enableVertexAttribArray(i);
      gl.vertexAttribPointer(i, 4, gl.FLOAT, false, 64, i * 16);
      gl.vertexAttribDivisor(i, 1);
    }
  }
  geometry() {
    this.appearance();
    const r = this.canvas.getBoundingClientRect();
    return {
      cols: Math.max(2, Math.min(400, Math.floor(r.width / this.cw))),
      rows: Math.max(1, Math.min(160, Math.floor(r.height / this.ch))),
    };
  }
  point(e: { clientX: number; clientY: number }, screen: Screen) {
    const r = this.canvas.getBoundingClientRect();
    return {
      x: Math.max(
        0,
        Math.min(screen.cols - 1, Math.floor((e.clientX - r.left) / this.cw)),
      ),
      y: Math.max(
        0,
        Math.min(screen.rows - 1, Math.floor((e.clientY - r.top) / this.ch)),
      ),
    };
  }
  glyph(cell: Cell): number {
    const key = this.glyphKey(cell);
    const old = this.glyphs.get(key);
    if (old !== undefined) return old;
    const slot = this.glyphs.size;
    if (slot >= this.capacity()) {
      this.glyphs.clear();
      this.atlasCtx.clearRect(0, 0, this.atlas.width, this.atlas.height);
      return this.glyph(cell);
    }
    this.glyphs.set(key, slot);
    this.atlasDirty = true;
    const columns = Math.floor(this.atlas.width / this.tileWidth());
    const x = (slot % columns) * this.tileWidth();
    const y = Math.floor(slot / columns) * this.tileHeight();
    const ctx = this.atlasCtx;
    ctx.clearRect(x, y, this.tileWidth(), this.tileHeight());
    ctx.fillStyle = "#fff";
    ctx.textBaseline = "alphabetic";
    ctx.font = `${cell.italic ? "italic " : ""}${cell.bold ? "bold " : ""}${this.fontSize * this.dpr}px ${this.family}`;
    ctx.save();
    ctx.beginPath();
    ctx.rect(x, y, this.tileWidth(), this.tileHeight());
    ctx.clip();
    if (
      !paintBlock(
        ctx,
        cell.text,
        x,
        y,
        this.cw * this.dpr * Math.max(1, cell.width),
        this.ch * this.dpr,
      )
    )
      ctx.fillText(
        cell.text,
        x,
        y + this.baseline * this.dpr,
        this.tileWidth(),
      );
    if (cell.underline)
      ctx.fillRect(
        x,
        y + this.tileHeight() - 2 * this.dpr,
        (this.tileWidth() / 2) * Math.max(1, cell.width),
        Math.max(1, this.dpr),
      );
    ctx.restore();
    return slot;
  }
  tileWidth() {
    return Math.ceil(this.cw * this.dpr) * 2;
  }
  tileHeight() {
    return Math.ceil(this.ch * this.dpr);
  }
  capacity() {
    return (
      Math.floor(this.atlas.width / this.tileWidth()) *
      Math.floor(this.atlas.height / this.tileHeight())
    );
  }
  glyphKey(cell: Cell) {
    return `${cell.bold ? "b" : ""}${cell.italic ? "i" : ""}${cell.underline ? "u" : ""}:${cell.width}:${cell.text}`;
  }
  draw(screen: Screen, selected: Selection = null, history?: string[]) {
    if (this.lost) return;
    this.appearance();
    const rect = this.canvas.getBoundingClientRect();
    const dpr = this.dpr;
    const w = Math.floor(rect.width * dpr),
      h = Math.floor(rect.height * dpr);
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w;
      this.canvas.height = h;
    }
    const bgDefault = this.background,
      fgDefault = this.foreground;
    if (history) {
      this.drawHistory(history, selected);
      return;
    }
    const start = selected
      ? Math.min(
          selected.start.y * screen.cols + selected.start.x,
          selected.end.y * screen.cols + selected.end.x,
        )
      : -1;
    const end = selected
      ? Math.max(
          selected.start.y * screen.cols + selected.start.x,
          selected.end.y * screen.cols + selected.end.x,
        )
      : -1;
    if (!this.gl) {
      const ctx = this.ctx!;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.fillStyle = bgDefault;
      ctx.fillRect(0, 0, rect.width, rect.height);
      screen.cells.forEach((cell, i) => {
        if (!cell.width) return;
        const x = (i % screen.cols) * this.cw,
          y = Math.floor(i / screen.cols) * this.ch;
        let bg =
          i >= start && i <= end
            ? this.selectionBackground
            : color(cell.bg, bgDefault, this.palette);
        let fg =
          i >= start && i <= end && this.selectionForeground
            ? this.selectionForeground
            : color(cell.fg, fgDefault, this.palette);
        const cursor =
          screen.cursor?.visible &&
          screen.cursor.position.x === i % screen.cols &&
          screen.cursor.position.y === Math.floor(i / screen.cols);
        ctx.fillStyle = bg;
        ctx.fillRect(x, y, this.cw * cell.width, this.ch);
        if (cursor) {
          ctx.fillStyle = this.cursor;
          ctx.fillRect(
            x,
            y + this.cursorTop,
            this.cw * cell.width,
            this.cursorHeight,
          );
          fg = this.cursorText;
        }
        ctx.fillStyle = fg;
        ctx.font = `${cell.italic ? "italic " : ""}${cell.bold ? "bold " : ""}${this.fontSize}px ${this.family}`;
        ctx.textBaseline = "alphabetic";
        if (!paintBlock(ctx, cell.text, x, y, this.cw * cell.width, this.ch))
          ctx.fillText(cell.text, x, y + this.baseline);
        if (
          cell.underline ||
          (cell.hyperlink && cell.hyperlink === this.hoverLink)
        )
          ctx.fillRect(x, y + this.ch - 2, this.cw * cell.width, 1);
      });
      return;
    }
    const gl = this.gl;
    gl.viewport(0, 0, w, h);
    gl.clearColor(...(rgba(bgDefault) as [number, number, number, number]));
    gl.clear(gl.COLOR_BUFFER_BIT);
    const cellDpr = dpr;
    const data: number[] = [];
    const columns = Math.floor(this.atlas.width / this.tileWidth());
    gl.useProgram(this.program!);
    gl.uniform2f(gl.getUniformLocation(this.program!, "viewport"), w, h);
    const cursor = screen.cursor?.visible ? screen.cursor.position : null;
    const cursorWidth = cursor
      ? Math.max(1, screen.cells[cursor.y * screen.cols + cursor.x]?.width || 1)
      : 0;
    gl.uniform4f(
      gl.getUniformLocation(this.program!, "cursorRect"),
      cursor ? cursor.x * this.cw * cellDpr : -1,
      cursor
        ? h -
            (cursor.y * this.ch + this.cursorTop + this.cursorHeight) * cellDpr
        : -1,
      cursorWidth * this.cw * cellDpr,
      cursor ? this.cursorHeight * cellDpr : 0,
    );
    gl.uniform4fv(
      gl.getUniformLocation(this.program!, "cursorBg"),
      rgba(this.cursor),
    );
    gl.uniform4fv(
      gl.getUniformLocation(this.program!, "cursorFg"),
      rgba(this.cursorText),
    );
    gl.bindTexture(gl.TEXTURE_2D, this.texture!);
    const flush = () => {
      if (!data.length) return;
      if (this.atlasDirty) {
        gl.texImage2D(
          gl.TEXTURE_2D,
          0,
          gl.RGBA,
          gl.RGBA,
          gl.UNSIGNED_BYTE,
          this.atlas,
        );
        this.atlasDirty = false;
      }
      gl.bindBuffer(gl.ARRAY_BUFFER, this.buffer!);
      gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(data), gl.DYNAMIC_DRAW);
      gl.drawArraysInstanced(gl.TRIANGLES, 0, 6, data.length / 16);
      data.length = 0;
    };
    screen.cells.forEach((cell, i) => {
      if (!cell.width) return;
      if (cell.hyperlink && cell.hyperlink === this.hoverLink)
        cell = { ...cell, underline: true };
      // Flush geometry before recycling a full atlas, otherwise cells already
      // queued in this frame would sample newly overwritten glyphs.
      const blank = !cell.underline && !cell.text.trim();
      if (
        !blank &&
        !this.glyphs.has(this.glyphKey(cell)) &&
        this.glyphs.size >= this.capacity()
      )
        flush();
      const slot = blank ? 0 : this.glyph(cell);
      let bg =
        i >= start && i <= end
          ? this.selectionBackground
          : color(cell.bg, bgDefault, this.palette);
      let fg =
        i >= start && i <= end && this.selectionForeground
          ? this.selectionForeground
          : color(cell.fg, fgDefault, this.palette);
      if (blank) fg = bg;
      data.push(
        (i % screen.cols) * this.cw * cellDpr,
        Math.floor(i / screen.cols) * this.ch * cellDpr,
        this.cw * cell.width * cellDpr,
        this.ch * cellDpr,
        ((slot % columns) * this.tileWidth()) / this.atlas.width,
        (Math.floor(slot / columns) * this.tileHeight()) / this.atlas.height,
        ((this.tileWidth() / 2) * cell.width) / this.atlas.width,
        this.tileHeight() / this.atlas.height,
        ...rgba(fg),
        ...rgba(bg),
      );
    });
    flush();
  }
  drawHistory(lines: string[], selected: Selection = null) {
    const geometry = this.geometry();
    if (this.historyLayout) {
      const old = this.historyCache;
      if (
        !old ||
        old.lines !== lines ||
        old.cols !== geometry.cols ||
        old.rows !== geometry.rows ||
        old.match !== this.historyMatch
      ) {
        const screen = this.historyLayout(lines, geometry.cols, geometry.rows);
        const match = this.historyMatch;
        if (match) {
          const first = match.start.y * screen.cols + match.start.x;
          const last = match.end.y * screen.cols + match.end.x;
          for (
            let i = Math.max(0, first);
            i <= Math.min(last, screen.cells.length - 1);
            i++
          ) {
            screen.cells[i].bg = { Rgb: [110, 84, 20] };
            screen.cells[i].fg = { Rgb: [255, 243, 180] };
          }
        }
        this.historyCache = {
          lines,
          cols: geometry.cols,
          rows: geometry.rows,
          match,
          screen,
        };
      }
      this.draw(this.historyCache!.screen, selected);
      return;
    }
    const blank: Cell = {
      text: " ",
      width: 1,
      fg: "Default",
      bg: "Default",
      bold: false,
      italic: false,
      underline: false,
    };
    const g = this.geometry();
    const cells: Cell[] = [];
    for (let y = 0; y < g.rows; y++) {
      const chars = Array.from(lines[y] || "");
      for (let x = 0; x < g.cols; x++)
        cells.push({ ...blank, text: chars[x] || " " });
    }
    this.draw({ cols: g.cols, rows: g.rows, cells, modes: {} }, selected);
  }
  clear() {
    if (this.lost) return;
    this.appearance();
    const rect = this.canvas.getBoundingClientRect();
    this.canvas.width = Math.max(1, Math.floor(rect.width * this.dpr));
    this.canvas.height = Math.max(1, Math.floor(rect.height * this.dpr));
    if (this.gl) {
      this.gl.viewport(0, 0, this.canvas.width, this.canvas.height);
      this.gl.clearColor(
        ...(rgba(this.background) as [number, number, number, number]),
      );
      this.gl.clear(this.gl.COLOR_BUFFER_BIT);
    } else if (this.ctx) {
      this.ctx.fillStyle = this.background;
      this.ctx.fillRect(0, 0, this.canvas.width, this.canvas.height);
    }
  }
  dispose() {
    const gl = this.gl;
    if (gl) {
      if (this.texture) gl.deleteTexture(this.texture);
      if (this.buffer) gl.deleteBuffer(this.buffer);
      if (this.program) gl.deleteProgram(this.program);
      gl.getExtension("WEBGL_lose_context")?.loseContext();
    }
  }
}
