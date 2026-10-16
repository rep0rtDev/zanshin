import { expect, test } from '@playwright/test';
import { login } from './helpers';

test('on a phone the sidebar is a drawer and Health is reachable', async ({ page }) => {
  await login(page);
  await expect(page.locator('.mob-bar')).toBeVisible();
  await expect(page.locator('.side')).not.toBeInViewport();
  await page.locator('.mob-menu').click();
  await expect(page.locator('.side a', { hasText: 'Health' })).toBeVisible();
  await page.locator('.side a', { hasText: 'Health' }).click();
  await expect(page.getByRole('heading', { name: /^Health/, level: 1 })).toBeVisible();
  await expect(page.locator('.side')).not.toBeInViewport();
  await expect(page.locator('.stat').first()).toBeVisible();
});

test('list pages stack rows and never overflow the phone viewport', async ({ page }) => {
  await login(page);
  for (const path of ['/console/flags', '/console/audiences', '/console/experiments', '/console/api-keys']) {
    await page.goto(path);
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
    await expect(page.locator('.table:not(.stack)')).toHaveCount(0);
    const [sw, cw] = await page.evaluate(() => [document.documentElement.scrollWidth, document.documentElement.clientWidth]);
    expect(sw, `${path} overflows horizontally`).toBeLessThanOrEqual(cw);
  }
});

test('row menu keeps its width on a phone', async ({ page }) => {
  await login(page);
  await page.goto('/console/flags');
  await page.locator('.table.stack .menu button').first().click();
  const pop = page.locator('.menu-pop').first();
  await expect(pop).toBeVisible();
  const box = await pop.boundingBox();
  expect(box!.width).toBeGreaterThan(120);
  const viewport = page.viewportSize()!;
  expect(box!.x + box!.width).toBeLessThanOrEqual(viewport.width);
});
