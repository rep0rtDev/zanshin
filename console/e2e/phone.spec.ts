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
