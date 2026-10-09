const { test, expect } = require("@playwright/test");

async function mockAccount(page) {
  await page.addInitScript(() => {
    window.calls = [];
    window.listeners = {};
    window.pairingApproved = false;
    let pending = false;
    const state = () => ({
      authenticated: localStorage.getItem("fixture-signed-in") === "yes",
      user: { id: "account-test", email: "tester@example.com" },
      pending,
      usage_mode: "unlimited",
      browser_login_available: true,
    });
    window.__TAURI__ = {
      event: { listen: async (name, fn) => { window.listeners[name] = fn; return () => {}; } },
      core: { invoke: async (command, args) => {
        window.calls.push({ command, args });
        switch (command) {
          case "account_status": return state();
          case "account_start_login": pending = true; return { user_code: "ABCD1234", verification_uri_complete: "https://www.roviumlabs.me/products/comrade?user_code=ABCD1234", expires_in: 600, interval: 3 };
          case "account_poll_login": if (window.pairingApproved) { localStorage.setItem("fixture-signed-in", "yes"); pending = false; } return state();
          case "account_open_login": case "account_open_page": return null;
          case "account_cancel_login": pending = false; return null;
          case "account_sign_in":
            if (args.password === "wrong") throw "ACCOUNT_AUTH: check your email and password";
            localStorage.setItem("fixture-signed-in", "yes"); return state();
          case "account_logout": localStorage.removeItem("fixture-signed-in"); return null;
          case "app_info": return { onboarded: true, memory_count: 0, memory_kinds: [] };
          case "get_prefs": return { browser: {}, coding: {}, voice: {} };
          case "voice_models_status": return { ready: true };
          case "browser_provision_status": return { ready: true };
          default: return [];
        }
      } },
    };
  });
}

test("signed-out app never mounts tools or reads local history", async ({ page }) => {
  await mockAccount(page); await page.goto("/");
  await expect(page.getByRole("heading", { name: "Sign in to Comrade" })).toBeVisible();
  await expect(page.locator("#composer")).toHaveCount(0);
  await expect(page.getByText("Unlimited access during internal testing")).toBeVisible();
  expect(await page.evaluate(() => window.calls.map(call => call.command))).toEqual(["account_status"]);
});

test("email sign-in handles rejection and clears the password before opening workspace", async ({ page }) => {
  await mockAccount(page); await page.goto("/");
  await page.getByLabel("Email", { exact: true }).fill("tester@example.com");
  await page.getByLabel("Password", { exact: true }).fill("wrong");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("check your email");
  await expect(page.getByLabel("Password", { exact: true })).toHaveValue("");
  await expect(page.locator("#composer")).toHaveCount(0);
  await page.getByLabel("Password", { exact: true }).fill("test-password");
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page.locator("#composer")).toBeVisible();
  await expect(page.getByText("tester@example.com", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => Object.keys(localStorage).some(key => /token|password/.test(key)))).toBe(false);
});

test("browser pairing waits for consent and cancellation stops polling", async ({ page }) => {
  await mockAccount(page); await page.goto("/");
  await page.getByRole("button", { name: "Continue in browser" }).click();
  await expect(page.getByText("ABCD1234", { exact: true })).toBeVisible();
  await expect(page.locator("#composer")).toHaveCount(0);
  expect(await page.evaluate(() => window.calls.find(call => call.command === "account_open_login")?.args)).toEqual({});
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(page.getByText("ABCD1234", { exact: true })).toHaveCount(0);
  await page.waitForTimeout(3300);
  expect(await page.evaluate(() => window.calls.filter(call => call.command === "account_poll_login").length)).toBe(0);
});

test("approved browser sign-in opens app and sign-out restores the gate", async ({ page }) => {
  await mockAccount(page); await page.goto("/");
  await page.getByRole("button", { name: "Continue in browser" }).click();
  await page.evaluate(() => { window.pairingApproved = true; });
  await expect(page.locator("#composer")).toBeVisible({ timeout: 8000 });
  await page.getByRole("button", { name: "Sign out", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Sign in to Comrade" })).toBeVisible();
  await expect(page.locator("#composer")).toHaveCount(0);
});

test("expired session locks app and account gate fits a narrow window", async ({ page }) => {
  await mockAccount(page);
  await page.addInitScript(() => localStorage.setItem("fixture-signed-in", "yes"));
  await page.goto("/");
  await expect(page.locator("#composer")).toBeVisible();
  await page.evaluate(() => window.listeners["account-event"]({ payload: { type: "signed_out" } }));
  await expect(page.locator("#composer")).toHaveCount(0);
  await expect(page.getByRole("alert")).toContainText("session ended");
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});
