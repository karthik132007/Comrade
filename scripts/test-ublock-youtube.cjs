// Local synthetic YouTube-origin responses in Comrade's actual Chromium.
// No traffic reaches YouTube: all requests are fulfilled by this fixture.
const assert = require('node:assert/strict');
const { chromium } = require('@playwright/test');
(async () => {
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${process.argv[2]}`);
  try {
    const context = browser.contexts()[0];
    const page = context.pages().find(p => !p.url().startsWith('chrome-extension://')) || await context.newPage();
    const payload = {
      adPlacements: [{adPlacementRenderer:{}}], adSlots: [{adSlotRenderer:{}}], playerAds: [{}],
      videoDetails: { videoId:'fixture-video', title:'Fixture content' },
      streamingData: { formats:[{url:'https://media.example/content.mp4'}] },
      playabilityStatus: {status:'OK'},
    };
    await context.route('**/*', route => {
      const url = new URL(route.request().url());
      if (url.pathname.includes('/youtubei/v1/player')) return route.fulfill({
        status:200, contentType:'application/json', body:JSON.stringify(payload),
      });
      if (url.hostname === 'www.youtube.com' && url.pathname === '/watch') return route.fulfill({
        status:200, contentType:'text/html', body:'<!doctype html><title>YouTube fixture</title><main>Content video</main>',
      });
      return route.fulfill({status:200, contentType:'text/plain', body:''});
    });
    await page.goto('https://www.youtube.com/watch?v=fixture-video');
    const result = await page.evaluate(async () => {
      const fetchData = await (await fetch('/youtubei/v1/player?fixture=fetch')).json();
      const xhrData = await new Promise((resolve, reject) => {
        const xhr = new XMLHttpRequest(); xhr.open('GET','/youtubei/v1/player?fixture=xhr');
        xhr.onload = () => { try { resolve(JSON.parse(xhr.responseText)); } catch (e) { reject(e); } };
        xhr.onerror = () => reject(new Error('XHR fixture failed')); xhr.send();
      });
      return {fetchData,xhrData};
    });
    const enabled = process.argv[3] !== 'off';
    for (const [path, data] of Object.entries(result)) {
      assert.equal(Boolean(data.adPlacements), !enabled, `${path}: adPlacements filtering`);
      assert.equal(Boolean(data.adSlots), !enabled, `${path}: adSlots filtering`);
      assert.deepEqual(data.videoDetails, payload.videoDetails, `${path}: content metadata preserved`);
      assert.deepEqual(data.streamingData, payload.streamingData, `${path}: playable streams preserved`);
      assert.deepEqual(data.playabilityStatus, payload.playabilityStatus, `${path}: playback remains OK`);
    }
    console.log(`YouTube-origin fetch and XHR scriptlets verified (protection ${enabled?'on':'off'}).`);
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode=1; });
