import { type APIRequestContext, expect, test } from "@playwright/test";

const MAILPIT = process.env.MAILPIT_URL ?? "http://localhost:8025";

function uniqueEmail() {
  return `e2e-${Date.now()}-${Math.random().toString(36).slice(2, 8)}@example.com`;
}

/** Waits for the sign-in email to arrive in Mailpit and returns it. */
async function signInEmail(request: APIRequestContext, to: string) {
  const search = () => request.get(`${MAILPIT}/api/v1/search`, { params: { query: `to:"${to}"` } });
  await expect.poll(async () => (await (await search()).json()).messages_count, { timeout: 10_000 }).toBe(1);
  const { messages } = await (await search()).json();
  const message = await (await request.get(`${MAILPIT}/api/v1/message/${messages[0].ID}`)).json();
  return {
    code: (message.Subject as string).slice(0, 6),
    link: (message.Text as string).match(/https?:\/\/\S+#token=\S+/)![0],
  };
}

async function requestSignIn(page: import("@playwright/test").Page, email: string) {
  await page.getByLabel("Email").fill(email);
  await page.getByRole("button", { name: "Continue with email" }).click();
  await expect(page.getByRole("heading", { name: "Check your email" })).toBeVisible();
}

test("signed-out visitors are sent to sign in, then back where they were going", async ({ page, request }) => {
  await page.goto("/playlists");
  await expect(page).toHaveURL(/\/login\?next=%2Fplaylists$/);

  const email = uniqueEmail();
  await requestSignIn(page, email);
  const { code } = await signInEmail(request, email);
  await page.getByLabel("6-digit code").fill(code); // submits automatically at 6 digits

  await expect(page).toHaveURL(/\/playlists$/);
  await expect(page.getByRole("heading", { name: "Playlists" })).toBeVisible();
});

test("a wrong code shows an error and can be retried", async ({ page, request }) => {
  const email = uniqueEmail();
  await page.goto("/login");
  await requestSignIn(page, email);
  const { code } = await signInEmail(request, email);

  await page.getByLabel("6-digit code").fill(code === "000000" ? "111111" : "000000");
  await expect(page.getByRole("alert")).toHaveText("incorrect code");
  await page.getByLabel("6-digit code").fill(code);
  await expect(page.getByRole("heading", { name: /^Welcome, e2e-/ })).toBeVisible();
});

test("the magic link signs in once, and the token leaves the URL", async ({ page, request }) => {
  const email = uniqueEmail();
  await page.goto("/login");
  await requestSignIn(page, email);
  const { link } = await signInEmail(request, email);

  await page.goto(link);
  await expect(page.getByRole("heading", { name: /^Welcome, e2e-/ })).toBeVisible();
  expect(page.url()).not.toContain("token");

  await page.goto(link);
  await expect(page.getByRole("heading", { name: "We couldn't sign you in" })).toBeVisible();
});

test("signing out ends the session", async ({ page, request }) => {
  const email = uniqueEmail();
  await page.goto("/login");
  await requestSignIn(page, email);
  await page.getByLabel("6-digit code").fill((await signInEmail(request, email)).code);
  await expect(page.getByRole("heading", { name: /^Welcome, e2e-/ })).toBeVisible();

  await page.getByRole("button", { name: "Account menu" }).click();
  await expect(page.getByRole("menu")).toContainText(email);
  await page.getByRole("menuitem", { name: "Sign out" }).click();
  await expect(page).toHaveURL(/\/login$/);

  await page.goto("/settings");
  await expect(page).toHaveURL(/\/login\?next=%2Fsettings$/);
});
