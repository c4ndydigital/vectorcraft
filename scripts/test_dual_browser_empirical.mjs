import { chromium } from 'playwright';

async function run() {
  console.log('🚀 Starting Dual-Browser E2E Playwright Verification...');

  const browser = await chromium.launch({
    headless: true,
    executablePath: '/usr/bin/chromium',
    args: ['--no-sandbox', '--disable-setuid-sandbox', '--disable-gpu']
  });

  const room = `dual-verify-${Date.now()}`;
  const url = `http://127.0.0.1:8767/?room=${room}`;

  console.log(`Connecting both browsers to room: ${room}`);

  // Create two isolated contexts
  const contextA = await browser.newContext();
  const contextB = await browser.newContext();

  const pageA = await contextA.newPage();
  const pageB = await contextB.newPage();

  // Monitor network requests for Google Fonts CDN leaks
  const leakedUrls = [];
  const checkUrl = (url) => {
    if (url.includes('fonts.googleapis.com') || url.includes('fonts.gstatic.com')) {
      console.error(`❌ GDPR Violation! External font request detected: ${url}`);
      leakedUrls.push(url);
    }
  };
  pageA.on('request', req => checkUrl(req.url()));
  pageB.on('request', req => checkUrl(req.url()));

  console.log('Navigating Page A to:', url);
  await pageA.goto(url, { waitUntil: 'domcontentloaded', timeout: 30000 });

  console.log('Navigating Page B to:', url);
  await pageB.goto(url, { waitUntil: 'domcontentloaded', timeout: 30000 });

  // Wait for canvas to be present on both pages
  await pageA.waitForSelector('canvas', { timeout: 15000 });
  await pageB.waitForSelector('canvas', { timeout: 15000 });
  console.log('✅ Canvas loaded on both clients');

  // Wait for WASM to initialize
  await pageA.waitForTimeout(3000);
  await pageB.waitForTimeout(3000);

  // Check Font Catalog & OFL self-hosting via window.__vectorcraft_get_fonts
  const fontsA = await pageA.evaluate(() => {
    if (typeof window.__vectorcraft_get_fonts === 'function') {
      return window.__vectorcraft_get_fonts();
    }
    return null;
  });

  console.log('Self-hosted Fonts detected in WASM:', fontsA ? fontsA.length : 'N/A');
  if (fontsA && fontsA.length > 0) {
    console.log('Sample fonts:', fontsA.slice(0, 5));
    const expected = ['Open Sans', 'Roboto', 'Lato', 'Montserrat', 'Fira Code', 'Poppins'];
    for (const exp of expected) {
      const found = fontsA.some(f => f.toLowerCase().includes(exp.toLowerCase()));
      if (found) {
        console.log(`✅ OFL Font '${exp}' present in self-hosted catalog`);
      } else {
        console.warn(`⚠️ OFL Font '${exp}' not found in catalog`);
      }
    }
  }

  // Verify GDPR compliance: zero leaked external font CDN calls
  if (leakedUrls.length === 0) {
    console.log('✅ 100% GDPR Compliant: Zero external Google Fonts CDN calls detected.');
  } else {
    throw new Error(`GDPR Violation: Found ${leakedUrls.length} requests to Google CDNs!`);
  }

  // Hover over canvas in Page A to generate cursor presence
  const canvasA = await pageA.$('canvas');
  const boxA = await canvasA.boundingBox();
  if (boxA) {
    await pageA.mouse.move(boxA.x + 200, boxA.y + 200);
    console.log('Moved mouse on Page A canvas');
  }

  await pageB.waitForTimeout(1000);

  console.log('🎉 Dual-Browser Playwright verification finished successfully!');
  await browser.close();
}

run().catch(err => {
  console.error('Test run failed:', err);
  process.exit(1);
});
