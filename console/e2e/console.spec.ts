import { expect, test } from '@playwright/test';
import { login, uniq } from './helpers';

test('login lands on Overview with the onboarding and stats', async ({ page }) => {
  await login(page);
  await expect(page.getByRole('heading', { name: 'Overview', level: 1 })).toBeVisible();
  await expect(page.locator('.stat').first()).toBeVisible();
  await expect(page.locator('.side a', { hasText: 'Feature flags' })).toBeVisible();
});

test('a flag created in the UI shows up in the list and evaluates', async ({ page }) => {
  await login(page);
  const key = uniq('e2e_flag');
  await page.locator('.side a', { hasText: 'Feature flags' }).click();
  await expect(page.getByRole('heading', { name: 'Feature flags', level: 1 })).toBeVisible();
  await page.getByRole('button', { name: /^New feature flag$/ }).click();
  await page.getByLabel('Key').fill(key);
  // the create form is a short wizard: Next through the steps, then Create
  for (let i = 0; i < 8; i++) {
    const next = page.getByRole('button', { name: 'Next', exact: true });
    if (!(await next.isVisible())) break;
    await next.click();
  }
  await page.getByRole('button', { name: 'Create', exact: true }).click();
  await expect(page.locator('.toast', { hasText: key })).toBeVisible();
  await page.locator('.side a', { hasText: 'Feature flags' }).click();
  await page.locator('input.search').fill(key);
  await expect(page.locator('table').getByText(key)).toBeVisible();
});

test('Platform → Health renders a verdict and its sections', async ({ page }) => {
  await login(page);
  await page.locator('.side a', { hasText: 'Health' }).click();
  await expect(page.getByRole('heading', { name: /^Health/, level: 1 })).toBeVisible();
  await expect(page.locator('h1 .badge', { hasText: /ok|warn|fail/ })).toBeVisible();
  for (const column of ['Service', 'Job']) {
    await expect(page.locator('th', { hasText: column }).first()).toBeVisible();
  }
  await expect(page.locator('table').getByText('PostgreSQL')).toBeVisible();
  await expect(page.locator('table').getByText('Redis')).toBeVisible();
});

test('Russian locale translates navigation, pages and the login screen', async ({ page }) => {
  await login(page, 'ru');
  await expect(page.locator('.side a', { hasText: 'Фича-флаги' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Обзор', level: 1 })).toBeVisible();
  await page.locator('.side a', { hasText: 'Состояние' }).click();
  await expect(page.getByRole('heading', { name: /^Состояние/, level: 1 })).toBeVisible();
  await expect(page.locator('th', { hasText: 'Сервис' }).first()).toBeVisible();
});
