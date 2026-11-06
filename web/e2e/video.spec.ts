import { expect, test } from "@playwright/test";
import { randomBytes } from "node:crypto";
import { writeFileSync } from "node:fs";
import path from "node:path";

import { CLIP, FIXTURES, signIn, upload } from "./helpers";

// Processing runs in the real worker (ffmpeg), so these tests take seconds, not milliseconds.
test.describe.configure({ timeout: 120_000 });

test("upload a video, watch it process live, and play it", async ({ page, request }) => {
  await signIn(page, request);
  await page.goto("/videos");
  await expect(page.getByRole("heading", { name: "No videos yet" })).toBeVisible();

  await upload(page, CLIP);
  const card = page.getByTestId("video-card").filter({ hasText: "Garden party" });
  // Becomes ready through server-sent events, without a reload.
  await expect(card).toHaveAttribute("data-status", "ready", { timeout: 90_000 });
  await expect(card).toContainText("0:05");

  await card.click();
  await expect(page).toHaveURL(/\/watch\//);
  await expect(page.getByText(/1280×720/)).toBeVisible();
  const video = page.locator("video");
  await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.readyState), { timeout: 20_000 }).toBeGreaterThanOrEqual(2);
  await video.evaluate((v: HTMLVideoElement) => {
    v.muted = true;
    return v.play();
  });
  await expect.poll(() => video.evaluate((v: HTMLVideoElement) => v.currentTime)).toBeGreaterThan(0.5);
  // hls.js loaded the 720p/480p/360p ladder.
  await expect
    .poll(() => page.evaluate(() => (document.querySelector("video") as HTMLVideoElement).videoHeight))
    .toBeGreaterThanOrEqual(360);
});

test("a video is private to its owner", async ({ browser, page, request }) => {
  await signIn(page, request);
  await page.goto("/videos");
  await upload(page, CLIP);
  await page.getByTestId("video-card").first().click();
  await expect(page).toHaveURL(/\/watch\//);
  const watchUrl = page.url();

  const stranger = await browser.newPage();
  await signIn(stranger, request);
  await stranger.goto(watchUrl);
  await expect(stranger.getByRole("heading", { name: "Page not found" })).toBeVisible();
  await stranger.goto("/videos");
  await expect(stranger.getByRole("heading", { name: "No videos yet" })).toBeVisible();
  await stranger.close();
});

test("delete, restore, then delete forever", async ({ page, request }) => {
  await signIn(page, request);
  await page.goto("/videos");
  await upload(page, CLIP);
  await page.getByTestId("video-card").first().click();
  await page.getByRole("button", { name: "Delete" }).click();
  await expect(page).toHaveURL(/\/videos$/);
  await expect(page.getByRole("heading", { name: "No videos yet" })).toBeVisible();

  await page.goto("/trash");
  await page.getByTestId("trash-row").getByRole("button", { name: "Restore" }).click();
  await expect(page.getByRole("heading", { name: "Trash is empty" })).toBeVisible();
  await page.goto("/videos");
  await expect(page.getByTestId("video-card")).toHaveCount(1);

  await page.getByTestId("video-card").first().click();
  await page.getByRole("button", { name: "Delete" }).click();
  // Wait for the delete to land: navigating away first would cancel the request.
  await expect(page).toHaveURL(/\/videos$/);
  await page.goto("/trash");
  page.once("dialog", (d) => d.accept());
  await page.getByTestId("trash-row").getByRole("button", { name: "Delete forever" }).click();
  await expect(page.getByRole("heading", { name: "Trash is empty" })).toBeVisible();

  await page.goto("/settings");
  await expect(page.getByTestId("storage-used")).toHaveText(/^0 B of 21\.5 GB used$/);
});

test("an upload interrupted by a page refresh resumes where it left off", async ({ page, request }) => {
  await signIn(page, request);
  // Not a real video: this test is about the upload, and the worker will (rightly) reject it.
  const file = path.join(FIXTURES, `interrupted-${Date.now()}.mp4`);
  writeFileSync(file, randomBytes(40 * 1024 * 1024));

  // Let exactly one part reach storage; hold every other part upload open.
  let allowed = 1;
  await page.route(/:9000\//, async (route) => {
    if (route.request().method() !== "PUT") return route.continue();
    if (allowed-- > 0) return route.continue();
    // Never answered: the refresh below cancels it.
  });
  await page.goto("/videos");
  await page.getByRole("banner").getByTestId("upload-input").setInputFiles(file);

  const mediaId = await expect
    .poll(() => page.evaluate(() => JSON.parse(localStorage.getItem("kintube.uploads.v1") ?? "[]")[0]?.mediaId))
    .toBeTruthy()
    .then(() => page.evaluate(() => JSON.parse(localStorage.getItem("kintube.uploads.v1")!)[0].mediaId as string));
  const status = () => page.request.get(`/api/uploads/${mediaId}`).then((r) => r.json());
  await expect.poll(async () => (await status()).uploaded_parts.length).toBe(1);
  expect((await status()).part_count).toBeGreaterThanOrEqual(3);

  await page.unrouteAll({ behavior: "ignoreErrors" });
  await page.reload();
  const row = page.getByTestId("upload-row");
  await expect(row).toHaveAttribute("data-status", "interrupted");
  await expect(row).toContainText("Choose the file again to resume");

  // Measured now: a held part may still have landed before the refresh cancelled it.
  const before = await status();
  expect(before.uploaded_parts.length).toBeGreaterThanOrEqual(1);
  const puts: string[] = [];
  page.on("request", (r) => {
    if (r.method() === "PUT" && r.url().includes(":9000/")) puts.push(r.url());
  });
  await row.getByTestId("resume-input").setInputFiles(file);
  await expect(row).toHaveAttribute("data-status", "done", { timeout: 60_000 });
  expect(puts).toHaveLength(before.part_count - before.uploaded_parts.length); // only the missing parts

  // The worker can't decode random bytes: the card shows a clear failure.
  await expect(page.getByTestId("video-card").first()).toHaveAttribute("data-status", "failed", { timeout: 60_000 });
});
