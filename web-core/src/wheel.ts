/** Accumulate trackpad fractions; DOM line/page deltas retain their units. */
export class WheelAccumulator {
  private remainder = 0;
  private lastAt = -Infinity;
  private direction = 0;
  push(
    delta: number,
    mode: number,
    lineHeight: number,
    pageRows: number,
    sensitivity = 1,
    immediate = false,
    now = performance.now(),
  ) {
    if (!Number.isFinite(delta)) return;
    const rows =
      mode === 1
        ? delta
        : mode === 2
          ? delta * pageRows
          : delta / Math.max(1, lineHeight);
    const direction = Math.sign(rows);
    if (!direction) return;
    const beginning = now - this.lastAt > 180 || direction !== this.direction;
    this.lastAt = now;
    this.direction = direction;
    if (beginning && immediate) this.remainder = 0;
    const amount =
      Math.max(-pageRows * 4, Math.min(pageRows * 4, rows)) / sensitivity;
    this.remainder +=
      beginning && immediate
        ? direction * Math.max(1, Math.abs(amount))
        : amount;
  }
  take(limit = 12) {
    const rows = Math.max(
      -limit,
      Math.min(
        limit,
        Math.trunc(this.remainder + Math.sign(this.remainder) * 1e-9),
      ),
    );
    this.remainder -= rows;
    return rows || 0;
  }
  get pending() {
    return Math.abs(this.remainder) >= 1;
  }
  reset() {
    this.remainder = 0;
    this.lastAt = -Infinity;
    this.direction = 0;
  }
}
