const { defineConfig } = require("@playwright/test");

module.exports = defineConfig({
  testDir: "./frontend/tests",
  fullyParallel: true,
  use: {
    baseURL: "http://127.0.0.1:1422",
    viewport: { width: 1180, height: 800 },
    launchOptions: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH
      ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH }
      : {},
  },
  webServer: {
    command:
      "npm run build && npm run preview -- --host 127.0.0.1 --port 1422 --strictPort",
    url: "http://127.0.0.1:1422",
    reuseExistingServer: false,
  },
});
