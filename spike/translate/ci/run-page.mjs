// Spike CI: open the spike page in a macOS browser, click Run, print window.__results.
// Usage: node run-page.mjs webkit|safari   (page served by ../serve.mjs on 127.0.0.1:8765)
const which = process.argv[2];
const URL = "http://127.0.0.1:8765/";
const poll = async (get) => {
  for (let i = 0; i < 240; i++) {
    const r = await get();
    if (r && (r.done || r.error)) return r;
    await new Promise((s) => setTimeout(s, 1000));
  }
  return { error: "timeout after 240 s" };
};
let results;
if (which === "webkit") {
  const { webkit } = await import("playwright");
  const browser = await webkit.launch();
  const page = await browser.newPage();
  page.on("console", (m) => console.log("console:", m.text()));
  await page.goto(URL);
  await page.click("#run");
  results = await poll(() => page.evaluate(() => window.__results));
  results.ua = await page.evaluate(() => navigator.userAgent);
  await browser.close();
} else {
  const { Builder, By } = await import("selenium-webdriver");
  const driver = await new Builder().forBrowser("safari").build();
  try {
    await driver.get(URL);
    await driver.findElement(By.id("run")).click();
    results = await poll(() => driver.executeScript("return window.__results || null"));
    results.ua = await driver.executeScript("return navigator.userAgent");
  } finally {
    await driver.quit();
  }
}
console.log(JSON.stringify(results, null, 2));
const ok = results.done && results.translations.length === 4 && results.translations.every((t) => t.out && t.same);
console.log(ok ? "RESULT: PASS" : "RESULT: FAIL");
process.exit(ok ? 0 : 1);
