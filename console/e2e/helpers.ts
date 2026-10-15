import { expect, type Page } from '@playwright/test';

export const ADMIN_KEY = process.env.ADMIN_KEY ?? 'e2e-bootstrap-admin-key-0001';

/** Signs in through the "Admin API key" tab exactly as an operator would. */
export async function login(page: Page, locale: 'en' | 'ru' = 'en') {
  await page.addInitScript((l) => localStorage.setItem('zanshin.locale', l), locale);
  await page.goto('/console/');
  await page.getByRole('button', { name: locale === 'ru' ? 'Админский API-ключ' : 'Admin API key' }).click();
  await page.locator('input[type=password]').fill(ADMIN_KEY);
  await page.keyboard.press('Enter');
  await expect(page.locator('.side')).toBeVisible();
}

export const uniq = (p: string) => `${p}_${Date.now().toString(36)}`;
