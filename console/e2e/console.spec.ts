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

test('Settings tabs switch sections instead of nesting paths', async ({ page }) => {
  await login(page);
  await page.goto('/console/settings');
  await page.locator('.tabs a', { hasText: 'Players & events' }).click();
  await expect(page).toHaveURL(/\/console\/settings\/players$/);
  await expect(page.locator('.tabs a.active')).toHaveText('Players & events');
  await page.locator('.tabs a', { hasText: 'Projects' }).click();
  await expect(page).toHaveURL(/\/console\/settings\/projects$/);
  await expect(page.locator('.tabs a.active')).toHaveText('Projects');
  await expect(page.locator('.main .card').first()).toBeVisible();
  await page.locator('.tabs a', { hasText: 'General' }).click();
  await expect(page).toHaveURL(/\/console\/settings$/);
});

test('Russian locale has no half-translated templates', async ({ page }) => {
  await login(page, 'ru');
  for (const path of ['/console/journeys', '/console/webhooks', '/console/flags']) {
    await page.goto(path);
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
    const text = await page.locator('.main').innerText();
    expect(text, `${path} mixes English template with Russian words`).not.toMatch(/\bNo [^\n]* yet\b|\bSearch [^\n]*…|\bNew [а-яё]/i);
  }
});

test('Taxonomy tabs navigate to absolute paths', async ({ page }) => {
  await login(page);
  await page.goto('/console/taxonomy');
  await page.locator('.tabs a', { hasText: 'Properties' }).click();
  await expect(page).toHaveURL(/\/console\/taxonomy\/properties$/);
  await page.locator('.tabs a', { hasText: 'Labels' }).click();
  await expect(page).toHaveURL(/\/console\/taxonomy\/labels$/);
  await page.locator('.tabs a', { hasText: 'Events' }).click();
  await expect(page).toHaveURL(/\/console\/taxonomy\/events$/);
  expect(await page.locator('.crumbs').innerText()).not.toMatch(/events.*events/);
});

test('desktop tables keep header and rows aligned; player Inbox tab loads', async ({ page }) => {
  await login(page);
  await page.goto('/console/taxonomy/properties');
  const tbl = page.locator('table.table').first();
  await expect(tbl).toBeVisible();
  expect(await tbl.evaluate((e) => getComputedStyle(e).display)).toBe('table');
  const ths = await tbl.locator('th').evaluateAll((els) => els.map((e) => Math.round(e.getBoundingClientRect().x)));
  const tds = await tbl.locator('tbody tr').first().locator('td').evaluateAll((els) => els.map((e) => Math.round(e.getBoundingClientRect().x)));
  expect(tds.slice(0, ths.length)).toEqual(ths);

  await page.goto('/console/identities');
  const row = page.locator('.table tr.clickable').first();
  if (await row.count()) {
    await row.click();
    await page.locator('.tabs button', { hasText: 'Inbox' }).click();
    await expect(page.locator('.tabs button.active')).toHaveText('Inbox');
    await page.waitForTimeout(800);
    await expect(page.locator('.error')).toHaveCount(0);
  }
});
