const { test, expect } = require("@playwright/test");

async function mockDesktop(page) {
  await page.addInitScript(() => {
    window.ipcCalls = [];
    window.agentListeners = {};
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
                url: "https://example.com",
                title: "Example",
              };
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


test('Brave ad blocking defaults on and stays disabled when other browser settings are saved', async ({ page }) => {
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
