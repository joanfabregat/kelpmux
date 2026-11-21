// Renders the installable web app's PNG icons from public/icons/icon.svg
// (full bleed, so iOS and Android maskable shapes crop only background).
// Run in the Playwright image after `npm ci`: node scripts/render-icons.mjs
// The PNGs are committed; rerun only when icon.svg changes.
import { chromium } from "playwright";
import { readFileSync, writeFileSync } from "node:fs";

const dir = new URL("../public/icons/", import.meta.url);
const svg = readFileSync(new URL("icon.svg", dir), "utf8");
const sizes = {
  "icon-192.png": 192,
  "icon-512.png": 512,
  "apple-touch-icon.png": 180,
};
const browser = await chromium.launch({ headless: true });
try {
  for (const [name, size] of Object.entries(sizes)) {
    const page = await browser.newPage({
      viewport: { width: size, height: size },
      deviceScaleFactor: 1,
    });
    await page.setContent(
      `<html><body style="margin:0">${svg.replace("<svg ", `<svg width="${size}" height="${size}" style="display:block" `)}</body></html>`,
    );
    writeFileSync(
      new URL(name, dir),
      await page.screenshot({ type: "png", omitBackground: false }),
    );
    await page.close();
  }
} finally {
  await browser.close();
}
console.log(`Rendered ${Object.keys(sizes).join(", ")}`);
