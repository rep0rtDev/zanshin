import { defineConfig, devices } from '@playwright/test';

// End-to-end tests against a running server that serves the built console (see .github/workflows/ci.yml → console-e2e).
export default defineConfig({
  testDir: './e2e',
  timeout: 30_000,
  retries: process.env.CI ? 1 : 0,
  workers: 1,
  reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : 'list',
  use: {
    baseURL: process.env.ZANSHIN_URL ?? 'http://127.0.0.1:7560',
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
    ...(process.env.PW_CHANNEL ? { channel: process.env.PW_CHANNEL } : {}),
    ...(process.env.PW_EXECUTABLE ? { launchOptions: { executablePath: process.env.PW_EXECUTABLE } } : {}),
  },
  projects: [
    { name: 'desktop', use: { ...devices['Desktop Chrome'] }, testIgnore: /phone\.spec\.ts/ },
    { name: 'phone', use: { ...devices['Pixel 5'] }, testMatch: /phone\.spec\.ts/ },
  ],
});
