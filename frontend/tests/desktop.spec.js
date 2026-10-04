const { test, expect } = require("@playwright/test");

async function mockDesktop(page) {
  await page.addInitScript(() => {
    window.ipcCalls = [];
    window.agentListeners = {};
    let browserTabs = [{ id: "tab-1", title: "DuckDuckGo", url: "https://duckduckgo.com/", active: true }];
    let prefs = {
      browser: { auto_show: true, width_pct: 45 },
      coding: { agents: ["codex"], default: "codex" },
      voice: {
        backend: "server",
        autoplay: true,
        mic: "",
        vad: { threshold: 0.5, silence_ms: 700 },
        tts: { voice: "0", speed: 1 },
      },
      server: {
        enabled: false,
        base_url: "",
        llm_model: "deepseek-flash",
        embedding_dim: 1536,
      },
    };
    window.__TAURI__ = {
      event: {
        listen: async (name, fn) => {
          window.agentListeners[name] = fn;
          return () => {};
        },
      },
      core: {
        invoke: async (command, args) => {
          window.ipcCalls.push({ command, args });
          switch (command) {
            case "app_info":
              return {
                onboarded: true,
                backend: "direct",
                provider: "deepseek",
                model: "deepseek-flash",
                memory_count: 0,
                memory_kinds: [],
              };
            case "get_prefs":
              return structuredClone(prefs);
            case "save_prefs":
              prefs = args.prefs;
              return prefs;
            case "get_audio_devices":
              return {
                inputs: [{ name: "Built-in microphone", is_default: true }],
              };
            case "voice_models_status":
              return { ready: true };
            case "system_coding_agents":
              return [
                {
                  id: "codex",
                  name: "Codex",
                  version: "1.0",
                  exec_supported: true,
                },
              ];
            case "send_message":
              return { status: "done", session_id: "test-session" };
            case "browser_state":
            case "browser_ensure":
              return {
                running: true,
                url: browserTabs.find((tab) => tab.active)?.url || "https://duckduckgo.com/",
                title: browserTabs.find((tab) => tab.active)?.title || "DuckDuckGo",
                tabs: structuredClone(browserTabs),
              };
            case "browser_tabs":
              return structuredClone(browserTabs);
            case "browser_tab_new": {
              browserTabs.forEach((tab) => { tab.active = false; });
              const tab = { id: `tab-${browserTabs.length + 1}`, title: "DuckDuckGo", url: args.url, active: true };
              browserTabs.push(tab);
              return structuredClone(tab);
            }
            case "browser_tab_select": {
              browserTabs.forEach((tab) => { tab.active = tab.id === args.id; });
              return structuredClone(browserTabs.find((tab) => tab.active));
            }
            case "browser_tab_close": {
              const wasActive = browserTabs.find((tab) => tab.id === args.id)?.active;
              browserTabs = browserTabs.filter((tab) => tab.id !== args.id);
              if (!browserTabs.length) browserTabs = [{ id: "tab-home", title: "DuckDuckGo", url: "https://duckduckgo.com/", active: true }];
              if (wasActive) browserTabs[0].active = true;
              return structuredClone(browserTabs.find((tab) => tab.active));
            }
            case "browser_open": {
              const active = browserTabs.find((tab) => tab.active);
              active.url = args.url.includes(" ") ? `https://duckduckgo.com/?q=${encodeURIComponent(args.url)}` : args.url;
              active.title = active.url.includes("duckduckgo.com") ? "DuckDuckGo" : "Page";
              return structuredClone(active);
            }
            case "browser_frame":
              return {
                data_url:
                  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=",
                width: 1280,
                height: 860,
              };
            default:
              return [];
          }
        },
      },
    };
  });
}

test("settings themes persist, categories retain edits, save reaches Tauri", async ({
  page,
}) => {
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockDesktop(page);
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "What’s on your mind?" }),
  ).toBeVisible();
  await expect(page.locator("body")).toHaveClass(/sidebar-open/);
  await expect.poll(() => page.locator('#app').evaluate(el => Math.round(el.getBoundingClientRect().x))).toBe(246);
  await page.screenshot({ path: "/tmp/comrade-mocha.png" });
  await page.getByRole("button", { name: "Open settings" }).click();
  const dialog = page.getByRole("dialog", { name: "Settings" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: /Latte/ }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "latte");
  await page.screenshot({ path: "/tmp/comrade-settings-latte.png" });
  await dialog.getByRole("button", { name: /Frappé/ }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "frappe");
  await dialog.getByRole("button", { name: /Mocha/ }).click();
  await page.screenshot({ path: "/tmp/comrade-settings-mocha.png" });
  await dialog.getByRole("button", { name: "Connections" }).click();
  await page.locator("#srv-url").fill("http://localhost:8000");
  await dialog.getByRole("button", { name: "Voice", exact: true }).click();
  await expect(page.locator("#voice-mic")).toContainText("Built-in microphone");
  await dialog
    .getByRole("button", { name: "Coding agents", exact: true })
    .click();
  await expect(page.locator("#code-agent-list")).toContainText("Codex");
  await dialog.getByRole("button", { name: "Connections" }).click();
  await expect(page.locator("#srv-url")).toHaveValue("http://localhost:8000");
  await dialog.getByRole("button", { name: "Save preferences" }).click();
  await expect(page.locator("#pref-status")).toHaveText(
    "saved to comrade.conf.",
  );
  expect(
    await page.evaluate(
      () =>
        window.ipcCalls.find((c) => c.command === "save_prefs").args.prefs
          .server.base_url,
    ),
  ).toBe("http://localhost:8000");
  await dialog.getByRole("button", { name: "Appearance", exact: true }).click();
  await dialog.getByRole("button", { name: /Latte/ }).click();
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(
    page.getByRole("button", { name: "Open settings" }),
  ).toBeFocused();
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "latte");
  expect(errors).toEqual([]);
});

test("browser has functional tabs and DuckDuckGo home/search controls", async ({ page }) => {
  await mockDesktop(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Toggle in-app browser" }).click();
  await expect(page.getByRole("tab", { name: /DuckDuckGo/ })).toHaveCount(1);
  await page.getByRole("button", { name: "New tab" }).click();
  await expect(page.getByRole("tab", { name: /DuckDuckGo/ })).toHaveCount(2);
  await expect(page.locator("#browser-url")).toHaveValue("https://duckduckgo.com/");
  await page.locator("#browser-url").fill("privacy focused search");
  await page.locator("#browser-form").press("Enter");
  await expect.poll(() => page.evaluate(() => window.ipcCalls.findLast((call) => call.command === "browser_open")?.args.url)).toBe("privacy focused search");
  await page.getByRole("button", { name: "Open DuckDuckGo home" }).click();
  await expect.poll(() => page.evaluate(() => window.ipcCalls.findLast((call) => call.command === "browser_open")?.args.url)).toBe("https://duckduckgo.com/");
  await page.getByRole("button", { name: /Close DuckDuckGo/ }).last().click();
  await expect(page.getByRole("tab", { name: /DuckDuckGo/ })).toHaveCount(1);
});

test("composer sends multiline messages and receives streamed events", async ({
  page,
}) => {
  await mockDesktop(page);
  await page.goto("/");
  await page.getByRole("button", { name: /Build something/ }).click();
  await expect(page.locator("#input")).toHaveValue(/coding task/);
  await page.locator("#input").fill("First line");
  await page.keyboard.press("Shift+Enter");
  await page.keyboard.type("Second line");
  await expect(page.locator("#input")).toHaveValue("First line\nSecond line");
  await page.keyboard.press("Enter");
  await expect(page.locator(".msg.user")).toContainText(
    "First line\nSecond line",
  );
  expect(
    await page.evaluate(
      () => window.ipcCalls.find((c) => c.command === "send_message").args.text,
    ),
  ).toBe("First line\nSecond line");
  await page.evaluate(() => {
    window.agentListeners["agent-event"]({
      payload: { type: "token", token: "Hello from Comrade." },
    });
  });
  await expect(page.locator("#messages")).toContainText("Hello from Comrade.");
  await page.getByRole("button", { name: "Toggle in-app browser" }).click();
  await expect(page.locator("#browser-pane")).toBeVisible();
  await page.getByRole("button", { name: "Toggle in-app browser" }).click();
  await page.getByRole("button", { name: /New chat/ }).click();
  await expect(page.locator(".msg")).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: /Build something/ }),
  ).toBeVisible();
});

test("small screens keep settings usable and keyboard focus inside the dialog", async ({
  page,
}) => {
  await mockDesktop(page);
  await page.setViewportSize({ width: 640, height: 600 });
  await page.goto("/");
  await page.getByRole("button", { name: "Open settings" }).click();
  const dialog = page.getByRole("dialog", { name: "Settings" });
  await page.screenshot({ path: "/tmp/comrade-settings-small.png" });
  await page.getByRole("button", { name: "Done", exact: true }).focus();
  await page.keyboard.press("Tab");
  await expect(
    page.getByRole("button", { name: "Close settings" }),
  ).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await expect(
    page.getByRole("button", { name: "Done", exact: true }),
  ).toBeFocused();
  const size = await dialog.boundingBox();
  expect(size.x).toBeGreaterThanOrEqual(0);
  expect(size.x + size.width).toBeLessThanOrEqual(640);
  expect(size.y + size.height).toBeLessThanOrEqual(600);
  await page.setViewportSize({ width: 420, height: 680 });
  await expect(dialog).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
});

test("web preview explains desktop requirements without a fake transcript", async ({
  page,
}) => {
  await page.goto("/");
  await expect(page.locator("#hint")).toContainText("Browser preview");
  await expect(page.locator(".msg")).toHaveCount(0);
});

async function mockStream(page) {
  await mockDesktop(page);
  await page.addInitScript(() => {
    const original = window.__TAURI__.core.invoke;
    window.streamChannels = {};
    window.streamSequence = 0;
    const frame = (streamId, sessionId, width, height) => ({
      type: 'frame', stream_id: streamId, session_id: sessionId, width, height,
      data_url: 'data:image/svg+xml;base64,' + btoa(`<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}"><rect width="100%" height="100%" fill="#e6e9ef"/><text x="20" y="50">Live browser frame ${sessionId}</text></svg>`),
    });
    window.pushStreamFrame = (id, sessionId) => window.streamChannels[id].onmessage(frame(id, sessionId, 640, 480));
    window.__TAURI__.core.Channel = class {};
    window.__TAURI__.core.invoke = async (command, args) => {
      if (!command.startsWith('browser_stream_')) return original(command, args);
      window.ipcCalls.push({ command, args });
      if (command === 'browser_stream_start') {
        const id = ++window.streamSequence;
        window.streamChannels[id] = args.onFrame;
        setTimeout(() => args.onFrame.onmessage(frame(id, id, args.width, args.height)), 0);
        return id;
      }
      if (command === 'browser_stream_resize') {
        window.streamChannels[args.streamId].onmessage(frame(args.streamId, 10, args.width, args.height));
      }
    };
  });
}

test('live pane streams without snapshot polling, forwards pointer input, resizes and stops when hidden', async ({ page }) => {
  await mockStream(page);
  await page.goto('/');
  await page.getByRole('button', { name: 'Toggle in-app browser' }).click();
  await expect(page.locator('#browser-img')).toHaveAttribute('src', /^data:image/);
  await expect.poll(() => page.evaluate(() => window.ipcCalls.filter(c => c.command === 'browser_stream_ack').length)).toBeGreaterThan(0);
  expect(await page.evaluate(() => window.ipcCalls.filter(c => c.command === 'browser_frame').length)).toBe(0);
  await page.locator('#browser-img').click({ position: { x: 100, y: 100 } });
  await page.keyboard.type('hello');
  await expect.poll(() => page.evaluate(() => window.ipcCalls.filter(c => c.command === 'browser_type_text').map(c => c.args.text).join(''))).toBe('hello');
  await page.keyboard.press('Control+A');
  await expect.poll(() => page.evaluate(() => window.ipcCalls.some(c => c.command === 'browser_press_key' && c.args.key.toLowerCase() === 'a' && c.args.modifiers === 2))).toBe(true);
  const pointers = await page.evaluate(() => window.ipcCalls.filter(c => c.command === 'browser_pointer').map(c => c.args.event));
  expect(pointers.some(e => e.type === 'mousePressed')).toBe(true);
  expect(pointers.some(e => e.type === 'mouseReleased')).toBe(true);
  await page.locator('#browser-img').hover({ position: { x: 100, y: 100 } });
  await page.mouse.wheel(0, 250);
  await expect.poll(() => page.evaluate(() => window.ipcCalls.some(c => c.command === 'browser_pointer' && c.args.event.type === 'mouseWheel'))).toBe(true);
  await page.setViewportSize({ width: 1300, height: 850 });
  await expect.poll(() => page.evaluate(() => window.ipcCalls.filter(c => c.command === 'browser_stream_resize').length)).toBeGreaterThan(0);
  await page.getByRole('button', { name: 'Toggle in-app browser' }).click();
  await expect.poll(() => page.evaluate(() => window.ipcCalls.filter(c => c.command === 'browser_stream_stop').length)).toBe(1);
  const before = await page.locator('#browser-img').getAttribute('src');
  await page.evaluate(() => window.pushStreamFrame(1, 99));
  expect(await page.locator('#browser-img').getAttribute('src')).toBe(before);
  await page.getByRole('button', { name: 'Toggle in-app browser' }).click();
  await expect.poll(() => page.evaluate(() => window.streamSequence)).toBe(2);
});

test('metadata polling preserves tab focus and closing a background tab keeps the live stream', async ({ page }) => {
  await mockStream(page);
  await page.goto('/');
  await page.getByRole('button', { name: 'Toggle in-app browser' }).click();
  const first = page.getByRole('tab').first();
  await first.focus();
  await expect.poll(() => page.evaluate(() => window.ipcCalls.filter(call => call.command === 'browser_state').length)).toBeGreaterThan(2);
  await expect(first).toBeFocused();
  expect(await page.evaluate(() => window.ipcCalls.filter(call => call.command === 'browser_tabs').length)).toBe(0);
  await page.getByRole('button', { name: 'New tab' }).click();
  await expect(page.getByRole('tab')).toHaveCount(2);
  const sequence = await page.evaluate(() => window.streamSequence);
  await page.getByRole('button', { name: /Close DuckDuckGo/ }).first().click();
  await expect(page.getByRole('tab')).toHaveCount(1);
  expect(await page.evaluate(() => window.streamSequence)).toBe(sequence);
  // Browser chrome shortcuts must not also reach the remote page.
  await page.locator('#browser-keys').focus();
  await page.keyboard.press('Control+l');
  await expect(page.locator('#browser-url')).toBeFocused();
  expect(await page.evaluate(() => window.ipcCalls.some(call => call.command === 'browser_press_key' && call.args.key === 'l'))).toBe(false);
});

test('tabs keep their positions and elements across reordered metadata and late replies', async ({ page }) => {
  await mockStream(page);
  await page.addInitScript(() => {
    const original = window.__TAURI__.core.invoke;
    let polls = 0;
    window.__TAURI__.core.invoke = async (command, args) => {
      const result = await original(command, args);
      if (command === 'browser_state') {
        if (window.holdBrowserSnapshot) {
          window.holdBrowserSnapshot = false;
          return new Promise(resolve => { window.releaseBrowserSnapshot = () => resolve(result); });
        }
        result.tabs.reverse();
        result.tabs.forEach(tab => { tab.title = `${tab.id} title ${++polls}`; });
      }
      return result;
    };
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Toggle in-app browser' }).click();
  await page.getByRole('button', { name: 'New tab', exact: true }).click();
  await expect(page.getByRole('tab')).toHaveCount(2);
  await page.evaluate(() => { window.savedFirstTab = document.querySelector('[data-tab-id="tab-1"]'); });
  await page.locator('[data-tab-id="tab-1"]').focus();
  await expect.poll(() => page.evaluate(() => window.ipcCalls.filter(call => call.command === 'browser_state').length)).toBeGreaterThan(4);
  expect(await page.getByRole('tab').evaluateAll(tabs => tabs.map(tab => tab.dataset.tabId))).toEqual(['tab-1', 'tab-2']);
  expect(await page.evaluate(() => window.savedFirstTab === document.querySelector('[data-tab-id="tab-1"]'))).toBe(true);
  await expect(page.locator('[data-tab-id="tab-1"]')).toBeFocused();
  await page.evaluate(() => { window.holdBrowserSnapshot = true; });
  await expect.poll(() => page.evaluate(() => typeof window.releaseBrowserSnapshot)).toBe('function');
  await page.getByRole('button', { name: 'New tab', exact: true }).click();
  await expect(page.getByRole('tab')).toHaveCount(3);
  await page.evaluate(() => window.releaseBrowserSnapshot());
  await expect(page.locator('[data-tab-id="tab-3"]')).toHaveAttribute('aria-selected', 'true');
  await expect.poll(() => page.evaluate(() => window.ipcCalls.filter(call => call.command === 'browser_state').length)).toBeGreaterThan(6);
  expect(await page.getByRole('tab').evaluateAll(tabs => tabs.map(tab => tab.dataset.tabId))).toEqual(['tab-1', 'tab-2', 'tab-3']);
});

test('rapid tab clicks finish on the last tab chosen', async ({ page }) => {
  await mockStream(page);
  await page.addInitScript(() => {
    const original = window.__TAURI__.core.invoke;
    window.__TAURI__.core.invoke = async (command, args) => {
      const result = await original(command, args);
      if (command === 'browser_tab_select' && window.holdTabSwitch) {
        window.holdTabSwitch = false;
        return new Promise(resolve => { window.releaseTabSwitch = () => resolve(result); });
      }
      return result;
    };
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Toggle in-app browser' }).click();
  for (const count of [2, 3]) {
    await page.getByRole('button', { name: 'New tab', exact: true }).click();
    await expect(page.getByRole('tab')).toHaveCount(count);
  }
  await page.evaluate(() => { window.holdTabSwitch = true; });
  await page.locator('[data-tab-id="tab-1"]').click();
  await expect.poll(() => page.evaluate(() => typeof window.releaseTabSwitch)).toBe('function');
  await page.locator('[data-tab-id="tab-2"]').click();
  await page.locator('[data-tab-id="tab-3"]').click();
  await page.evaluate(() => window.releaseTabSwitch());
  await expect.poll(() => page.evaluate(() => window.ipcCalls.filter(call => call.command === 'browser_tab_select').map(call => call.args.id))).toEqual(['tab-1', 'tab-3']);
  await expect(page.locator('[data-tab-id="tab-3"]')).toHaveAttribute('aria-selected', 'true');
  expect(await page.getByRole('tab').evaluateAll(tabs => tabs.map(tab => tab.dataset.tabId))).toEqual(['tab-1', 'tab-2', 'tab-3']);
});

test('tabs display site favicons, reuse loaded images and show a globe when an icon fails', async ({ page }) => {
  await mockStream(page);
  await page.addInitScript(() => {
    const original = window.__TAURI__.core.invoke;
    window.__TAURI__.core.invoke = async (command, args) => {
      const result = await original(command, args);
      const tabs = command === 'browser_tabs' ? result : result.tabs;
      if (Array.isArray(tabs)) tabs.forEach(tab => {
        tab.favicon_url = window.failSiteIcon ? 'https://icons.example/broken.svg' : 'https://icons.example/site.svg';
      });
      return result;
    };
  });
  await page.route('https://icons.example/**', route => route.request().url().endsWith('/site.svg')
    ? route.fulfill({ contentType: 'image/svg+xml', body: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect width="24" height="24" fill="red"/></svg>' })
    : route.abort());
  await page.route('https://duckduckgo.com/favicon.ico', route => route.abort());
  await page.goto('/');
  await page.getByRole('button', { name: 'Toggle in-app browser' }).click();
  const icon = page.locator('.browser-tab-favicon img');
  await expect(icon).toHaveAttribute('src', 'https://icons.example/site.svg');
  await expect(page.locator('.browser-tab-favicon')).toHaveClass(/has-icon/);
  await page.evaluate(() => { window.savedFavicon = document.querySelector('.browser-tab-favicon img'); });
  await expect.poll(() => page.evaluate(() => window.ipcCalls.filter(call => call.command === 'browser_state').length)).toBeGreaterThan(2);
  expect(await page.evaluate(() => window.savedFavicon === document.querySelector('.browser-tab-favicon img'))).toBe(true);
  await page.evaluate(() => { window.failSiteIcon = true; });
  await expect(page.locator('.browser-tab-favicon img')).toHaveCount(0);
  await expect(page.locator('.browser-tab-favicon > svg')).toBeVisible();
  await expect(page.locator('.browser-tab-favicon')).toHaveText('');
});


test('browser-first layout keeps streaming, floats chat on hover and persists pinned chat', async ({ page }) => {
  await mockStream(page);
  await page.goto('/');
  await page.getByRole('button', { name: 'Toggle in-app browser' }).click();
  await expect(page.locator('#browser-img')).toHaveAttribute('src', /^data:image/);
  await page.getByRole('button', { name: 'Make browser the main view' }).click();
  await expect(page.locator('body')).toHaveClass(/browser-primary/);
  await expect(page.locator('#sidebar')).toBeHidden();
  // Expanding can place the pointer over the newly positioned Chat button.
  await page.locator('#browser-url').hover();
  await expect(page.locator('#chat-workspace')).toBeHidden();
  await expect.poll(async () => (await page.locator('#browser-view').boundingBox()).width).toBeGreaterThan(1100);
  await expect.poll(() => page.evaluate(() => window.ipcCalls.some(c => c.command === 'browser_stream_resize' && c.args.width > 1100))).toBe(true);
  await page.getByRole('button', { name: 'Show chat panel' }).hover();
  await expect(page.locator('#chat-workspace')).toBeVisible();
  await page.locator('#browser-url').hover();
  await expect(page.locator('#chat-workspace')).toBeHidden();
  await page.getByRole('button', { name: 'Show chat panel' }).click();
  await page.locator('#input').fill('Keep this draft');
  await page.locator('#browser-url').hover();
  await expect(page.locator('#chat-workspace')).toBeVisible();
  await page.screenshot({ path: '/tmp/comrade-browser-floating.png' });
  await page.getByRole('button', { name: 'Pin chat beside browser' }).click();
  await expect(page.locator('body')).toHaveClass(/chat-pinned/);
  const pinned = await page.locator('#chat-workspace').boundingBox();
  const resized = await page.locator('#browser-pane').boundingBox();
  expect(resized.x + resized.width).toBeLessThanOrEqual(pinned.x + 1);
  await expect(page.locator('#input')).toHaveValue('Keep this draft');
  expect(await page.evaluate(() => window.streamSequence)).toBe(1);
  expect(await page.evaluate(() => window.ipcCalls.filter(c => c.command === 'browser_stream_stop').length)).toBe(0);
  await page.screenshot({ path: '/tmp/comrade-browser-pinned.png' });
  await page.getByRole('button', { name: 'Open settings' }).click();
  await expect(page.getByRole('dialog', { name: 'Settings' })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog', { name: 'Settings' })).toBeHidden();
  await expect(page.locator('body')).toHaveClass(/chat-pinned/);
  await page.reload();
  await expect(page.locator('body')).toHaveClass(/browser-primary/);
  await expect(page.locator('body')).toHaveClass(/chat-pinned/);
  await expect(page.locator('#browser-img')).toHaveAttribute('src', /^data:image/);
  await page.setViewportSize({ width: 640, height: 600 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.getByRole('button', { name: 'Close chat panel' }).click();
  await expect(page.locator('#chat-workspace')).toBeHidden();
  await page.keyboard.press('Escape');
  await expect(page.locator('body')).not.toHaveClass(/browser-primary/);
  await expect(page.locator('#chat-workspace')).toBeVisible();
});

test('desktop fullscreen uses the native window and Escape restores the chat layout', async ({ page }) => {
  await mockStream(page);
  await page.addInitScript(() => {
    window.fullscreenCalls = [];
    window.__TAURI__.window = { getCurrentWindow: () => ({
      setFullscreen: async (enabled) => { window.fullscreenCalls.push(enabled); },
    }) };
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Toggle in-app browser' }).click();
  await page.getByRole('button', { name: 'Enter fullscreen' }).click();
  await expect(page.getByRole('button', { name: 'Exit fullscreen' })).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('body')).toHaveClass(/browser-primary/);
  await page.keyboard.press('Escape');
  await expect(page.getByRole('button', { name: 'Enter fullscreen' })).toHaveAttribute('aria-pressed', 'false');
  expect(await page.evaluate(() => window.fullscreenCalls)).toEqual([true, false]);
  await expect(page.locator('body')).not.toHaveClass(/browser-primary/);
  await page.keyboard.press('F11');
  await expect(page.locator('body')).toHaveClass(/browser-primary/);
  await page.getByRole('button', { name: 'Hide browser', exact: true }).click();
  await expect(page.locator('#browser-pane')).toBeHidden();
  await expect(page.locator('body')).not.toHaveClass(/browser-primary/);
  await expect.poll(() => page.evaluate(() => window.fullscreenCalls)).toEqual([true, false, true, false]);
});


test('uBlock Origin Lite blocking defaults on and stays disabled when other browser settings are saved', async ({ page }) => {
  await mockDesktop(page); // Legacy prefs omit adblock_enabled; default must be on.
  await page.goto('/');
  await page.getByRole('button', { name: 'Open settings' }).click();
  const dialog = page.getByRole('dialog', { name: 'Settings' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: 'Browser', exact: true }).click();
  const protection = page.getByRole('checkbox', { name: 'Block ads and trackers' });
  await expect(protection).toBeChecked();
  await protection.uncheck();
  await expect.poll(() => page.evaluate(() => window.ipcCalls.filter(c => c.command === 'save_prefs').at(-1)?.args.prefs.browser.adblock_enabled)).toBe(false);
  await page.locator('#pref-bwidth').focus();
  await page.keyboard.press('ArrowRight');
  await expect.poll(() => page.evaluate(() => window.ipcCalls.filter(c => c.command === 'save_prefs').at(-1)?.args.prefs.browser.width_pct)).toBe(46);
  expect(await page.evaluate(() => window.ipcCalls.filter(c => c.command === 'save_prefs').at(-1).args.prefs.browser.adblock_enabled)).toBe(false);
  await dialog.getByRole('button', { name: 'Save preferences' }).click();
  await expect(page.locator('#pref-status')).toHaveText('saved to comrade.conf.');
  expect(await page.evaluate(() => window.ipcCalls.filter(c => c.command === 'save_prefs').at(-1).args.prefs.browser.adblock_enabled)).toBe(false);
  await protection.check();
  await expect.poll(() => page.evaluate(() => window.ipcCalls.filter(c => c.command === 'save_prefs').at(-1)?.args.prefs.browser.adblock_enabled)).toBe(true);
});

async function mockCodingBoard(page, ready = true) {
  await mockDesktop(page);
  await page.addInitScript(({ ready }) => {
    window.codingRuns = [];
    window.projectPickerResults = [{path:"/projects/app"}];
    const original = window.__TAURI__.core.invoke;
    window.__TAURI__.core.invoke = async (command, args) => {
      if (!command.startsWith("coding_")) return original(command, args);
      window.ipcCalls.push({ command, args });
      switch (command) {
        case "coding_pick_project": { const result = window.projectPickerResults.shift(); if (result?.error) throw result.error; return result || null; }
        case "coding_runtime": return { ready, message: ready ? "Installed agents use their existing logins and permissions." : "No enabled coding agents found.", agents: [
          { id:"codex", name:"Codex", available:true, enabled:true }, { id:"opencode", name:"OpenCode", available:true, enabled:true }, { id:"hermes", name:"Hermes", available:false, enabled:true },
        ] };
        case "coding_runs": return structuredClone(window.codingRuns).map(r => ({...r,jobs:r.jobs.map(j => ({...j,log:""}))}));
        case "coding_run": return structuredClone(window.codingRuns.find(r => r.id === args.runId));
        case "coding_start": {
          const run = {id:"run-test",plan:args.plan,created_at:Date.now(),status:"running",jobs:args.plan.tasks.map(spec=>({spec,status:"queued",log:"",error:null}))};
          window.codingRuns.push(run); return structuredClone(run);
        }
        case "coding_cancel": {
          const run=window.codingRuns.find(r=>r.id===args.runId); run.status="cancelled"; run.jobs.forEach(j => { if (!args.jobId || j.spec.id===args.jobId) j.status="cancelled"; }); return structuredClone(run);
        }
      }
    };
  }, { ready });
}

test("coding board starts multiple agents with explicit dependencies", async ({ page }) => {
  await mockCodingBoard(page); await page.goto("/"); await page.getByRole("button", {name:"Comrade Orch",exact:true}).click();
  const board=page.getByRole("region",{name:"Comrade Orch",exact:true});
  await board.getByRole("button",{name:"Choose project directory",exact:true}).click();
  await board.getByLabel("Goal",{exact:true}).fill("Build settings and verify"); await expect(board.getByLabel("Project directory")).toHaveValue("/projects/app");
  await board.getByLabel("Task",{exact:true}).fill("Build the settings page"); await board.getByLabel("Agent",{exact:true}).selectOption("codex");
  await board.getByRole("button",{name:"Add task",exact:true}).click(); const second=board.locator("fieldset").nth(1);
  await second.getByLabel("Task",{exact:true}).fill("Review and test the settings page"); await second.getByLabel("Agent",{exact:true}).selectOption("opencode"); await second.getByLabel("task-1",{exact:true}).check();
  await expect(board.getByText("Agents work directly in your project", {exact:false})).toBeVisible();
  await page.screenshot({path:"/tmp/comrade-orchestration-board.png"});
  await board.getByRole("button",{name:"Start run",exact:true}).click(); await expect(board.getByRole("heading",{name:"Build settings and verify"})).toBeVisible();
  const call=await page.evaluate(()=>window.ipcCalls.find(c=>c.command==="coding_start"));
  expect(call.args.plan).not.toHaveProperty("network"); expect(call.args.plan.tasks[1].depends_on).toEqual(["task-1"]); expect(call.args.plan.tasks.map(t=>t.agent)).toEqual(["codex","opencode"]);
  await board.getByRole("button",{name:"Stop run",exact:true}).click(); await expect(board.locator(".agent-state.cancelled").first()).toBeVisible();
  expect(await page.evaluate(()=>window.ipcCalls.some(c=>c.command==="coding_apply"))).toBe(false);
});

test("coding jobs stream logs, show completion and return to chat", async ({ page }) => {
  await mockCodingBoard(page); await page.goto("/");
  await page.evaluate(()=>window.codingRuns.push({id:"run-monitor",created_at:Date.now(),plan:{title:"Add feature",working_dir:"/projects/app",max_parallel:2,timeout_secs:600},status:"running",jobs:[{spec:{id:"task-1",agent:"codex",task:"Implement feature",model:"",depends_on:[]},status:"running",log:"Reading project"}]}));
  await page.getByRole("button",{name:"Comrade Orch",exact:true}).click(); const board=page.getByRole("region",{name:"Comrade Orch",exact:true});
  await board.getByRole("button",{name:/Add feature/}).click(); await board.locator("summary").click(); await expect(board.locator("pre")).toContainText("Reading project");
  await page.evaluate(()=>{ const run=window.codingRuns[0]; run.status="succeeded"; Object.assign(run.jobs[0],{status:"succeeded",log:"Tests: 5 passed"}); });
  await expect(board.locator("pre")).toContainText("Tests: 5 passed");
  await expect(board.getByText("Agent finished. See output for checks and results.")).toBeVisible();
  await expect(board.getByRole("button",{name:"Stop job",exact:true})).toHaveCount(0);
  await page.screenshot({path:"/tmp/comrade-native-monitor.png"});
  await page.keyboard.press("Escape"); await expect(board).toBeHidden(); await expect(page.getByRole("button",{name:"Comrade Orch",exact:true})).toBeFocused();
});

test("coding board detects missing local agents and can hand planning to Comrade", async ({ page }) => {
  await mockCodingBoard(page,false); await page.goto("/"); await page.getByRole("button",{name:"Comrade Orch",exact:true}).click(); const board=page.getByRole("region",{name:"Comrade Orch",exact:true});
  await board.getByRole("button",{name:"Choose project directory",exact:true}).click();
  await expect(board.getByRole("status")).toContainText("No enabled coding agents found"); await expect(board.getByRole("button",{name:"Start run",exact:true})).toBeDisabled();
  await board.getByLabel("Goal",{exact:true}).fill("Build auth"); await expect(board.getByLabel("Project directory")).toHaveValue("/projects/app");
  await board.getByRole("button",{name:"Let Comrade plan",exact:true}).click();
  await expect(board).toBeHidden();
  await expect.poll(() => page.evaluate(() => window.ipcCalls.filter(c => c.command === "send_message").length)).toBe(1);
  const handoff = await page.evaluate(() => window.ipcCalls.find(c => c.command === "send_message"));
  expect(handoff.args.text).toBe("Build auth"); expect(handoff.args.projectDir).toBe("/projects/app");
  await expect(page.locator("#messages")).toContainText("Build auth");
  await expect(page.locator("#messages")).not.toContainText("coding.startPlan");
  await page.locator("#input").fill("yes"); await page.getByRole("button",{name:"Send message",exact:true}).click();
  const followup = await page.evaluate(() => window.ipcCalls.filter(c => c.command === "send_message").at(-1));
  expect(followup.args.sessionId).toBe("test-session"); expect(followup.args.text).toBe("yes");
  expect(followup.args.projectDir).toBeNull();
  expect(await page.evaluate(()=>window.ipcCalls.some(c=>c.command==="coding_start"))).toBe(false);
});

test("Comrade Orch sits below New chat and remembers selected project directories", async ({ page }) => {
  await mockCodingBoard(page); await page.goto("/");
  const sidebar = page.locator("#sidebar");
  const newChat = await sidebar.getByRole("button", { name: /New chat/ }).boundingBox();
  const orch = await sidebar.getByRole("button", { name: "Comrade Orch", exact: true }).boundingBox();
  expect(orch.y).toBeGreaterThan(newChat.y + newChat.height);
  expect(await page.locator(".head-right").getByRole("button", { name: "Comrade Orch", exact: true }).count()).toBe(0);
  await sidebar.getByRole("button", { name: "Add project", exact: true }).click();
  const workspace = page.getByRole("region", { name: "Comrade Orch", exact: true });
  await expect(workspace).toBeVisible(); await expect(page.locator("#workarea")).toBeHidden();
  await expect(workspace.getByLabel("Project directory")).toHaveValue("/projects/app");
  await expect(sidebar.getByRole("button", { name: "Open project app", exact: true })).toBeVisible();
  expect(await page.evaluate(() => window.ipcCalls.find(c => c.command === "coding_pick_project").args)).toEqual({ initialDir: null });
  await page.reload();
  await sidebar.getByRole("button", { name: "Open project app", exact: true }).click();
  await expect(workspace.getByLabel("Project directory")).toHaveValue("/projects/app");
  await sidebar.getByRole("button", { name: /New chat/ }).click();
  await expect(workspace).toBeHidden(); await expect(page.locator("#workarea")).toBeVisible();
  await expect(page.locator("#input")).toBeFocused();
});

test("project switching scopes run history and picker cancellation preserves the selection", async ({ page }) => {
  await mockCodingBoard(page); await page.goto("/");
  await page.getByRole("button", { name: "Add project", exact: true }).click();
  const workspace = page.getByRole("region", { name: "Comrade Orch", exact: true });
  await workspace.getByLabel("Goal", { exact: true }).fill("App changes");
  await workspace.getByLabel("Task", { exact: true }).fill("Implement app change");
  await workspace.getByRole("button", { name: "Start run", exact: true }).click();
  await expect(workspace.getByRole("heading", { name: "App changes", exact: true })).toBeVisible();
  await page.evaluate(() => window.projectPickerResults.push({path:"/projects/service"}));
  await page.getByRole("button", { name: "Add project", exact: true }).click();
  await expect(workspace.getByLabel("Project directory")).toHaveValue("/projects/service");
  await expect(workspace.getByLabel("Goal", { exact: true })).toHaveValue("");
  await expect(workspace.getByRole("button", { name: /App changes/ })).toHaveCount(0);
  await workspace.getByRole("button", { name: "Change directory", exact: true }).click();
  await expect(workspace.getByLabel("Project directory")).toHaveValue("/projects/service");
  expect(await page.evaluate(() => window.ipcCalls.filter(c => c.command === "coding_pick_project").at(-1).args)).toEqual({ initialDir:"/projects/service" });
  expect(await page.evaluate(() => window.ipcCalls.some(c => c.command === "coding_cancel"))).toBe(false);
  await page.getByRole("button", { name: "Open project app", exact: true }).click();
  await expect(workspace.getByRole("button", { name: /App changes/ })).toBeVisible();
  await workspace.getByRole("button", { name: /App changes/ }).click();
  await expect(workspace.getByRole("button", { name: "Stop run", exact: true })).toBeVisible();
  await page.screenshot({path:"/tmp/comrade-orch-project-workspace.png"});
});

test("cancelled and failed directory selection never starts work", async ({ page }) => {
  await mockCodingBoard(page); await page.goto("/");
  await page.evaluate(() => { window.projectPickerResults = [null, {error:"Directory access unavailable"}]; });
  await page.getByRole("button", { name: "Add project", exact: true }).click();
  const workspace = page.getByRole("region", { name: "Comrade Orch", exact: true });
  await expect(workspace.getByRole("heading", { name:"Start with your project" })).toBeVisible();
  await expect(workspace.getByRole("button", { name:"Start run", exact:true })).toHaveCount(0);
  await workspace.getByRole("button", {name:"Choose project directory",exact:true}).click();
  await expect(workspace.getByRole("alert")).toContainText("Directory access unavailable");
  expect(await page.evaluate(() => window.ipcCalls.some(c => c.command === "coding_start"))).toBe(false);
});

test("orchestration stays accessible from browser view and New chat shortcut returns to chat", async ({ page }) => {
  await mockCodingBoard(page); await page.goto("/");
  await page.getByRole("button",{name:"Toggle in-app browser",exact:true}).click();
  await page.getByRole("button",{name:"Make browser the main view",exact:true}).click();
  await expect(page.locator("#sidebar")).toBeHidden();
  await page.getByRole("button",{name:"Toggle chat sidebar",exact:true}).click();
  await page.getByRole("button",{name:"Add project",exact:true}).click();
  const workspace = page.getByRole("region",{name:"Comrade Orch",exact:true});
  await expect(workspace.getByLabel("Project directory")).toHaveValue("/projects/app");
  await workspace.getByLabel("Goal",{exact:true}).focus(); await page.keyboard.press("Control+l");
  await expect(workspace.getByLabel("Goal",{exact:true})).toBeFocused();
  await page.keyboard.press("Control+n");
  await expect(workspace).toBeHidden(); await expect(page.locator("#input")).toBeFocused();
});

test("project workspace fits a small window and sidebar selection remains reachable", async ({ page }) => {
  await page.setViewportSize({width:390,height:844}); await mockCodingBoard(page); await page.goto("/");
  await page.getByRole("button",{name:"Add project",exact:true}).click();
  const workspace = page.getByRole("region",{name:"Comrade Orch",exact:true});
  await expect(page.locator("#sidebar")).toBeHidden();
  await expect(workspace.getByLabel("Project directory")).toHaveValue("/projects/app");
  const bounds = await workspace.boundingBox(); expect(bounds.x).toBeGreaterThanOrEqual(0); expect(bounds.width).toBeLessThanOrEqual(390);
  await expect(workspace.getByRole("button",{name:"Change directory",exact:true})).toBeVisible();
  await page.getByRole("button",{name:"Toggle chat sidebar",exact:true}).click();
  await expect(page.getByRole("button",{name:"Open project app",exact:true})).toBeVisible();
  await page.getByRole("button",{name:"Open project app",exact:true}).click();
  await expect(page.locator("#sidebar")).toBeHidden();
  await page.screenshot({path:"/tmp/comrade-orch-small-window.png"});
});

test("a late orchestration reply cannot restore its session after New chat", async ({ page }) => {
  await mockCodingBoard(page);
  await page.addInitScript(() => {
    const original = window.__TAURI__.core.invoke;
    window.__TAURI__.core.invoke = (command, args) => {
      if (command === "send_message" && args.projectDir) {
        window.ipcCalls.push({command,args});
        return new Promise(resolve => { window.finishOldHandoff = () => resolve({status:"done",session_id:"old-project-session"}); });
      }
      return original(command,args);
    };
  });
  await page.goto("/"); await page.getByRole("button",{name:"Add project",exact:true}).click();
  const board=page.getByRole("region",{name:"Comrade Orch",exact:true});
  await board.getByLabel("Goal",{exact:true}).fill("Add paper and ink theme");
  await board.getByRole("button",{name:"Let Comrade plan",exact:true}).click();
  await expect.poll(() => page.evaluate(() => typeof window.finishOldHandoff)).toBe("function");
  await page.getByRole("button",{name:/New chat/}).click();
  await page.evaluate(() => window.finishOldHandoff());
  await expect.poll(() => page.evaluate(() => window.localStorage.getItem("comrade-session"))).toBeNull();
  await page.locator("#input").fill("Hello"); await page.getByRole("button",{name:"Send message",exact:true}).click();
  const latest = await page.evaluate(() => window.ipcCalls.filter(c=>c.command==="send_message").at(-1));
  expect(latest.args.sessionId).toBeNull(); expect(latest.args.projectDir).toBeNull();
});
