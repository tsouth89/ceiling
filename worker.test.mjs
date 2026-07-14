import assert from "node:assert/strict";
import test from "node:test";

import worker, { aggregateReleases, safePath } from "./worker.mjs";

test("aggregateReleases counts executable downloads without checksum assets", () => {
  const result = aggregateReleases([
    {
      tag_name: "v0.43.2",
      name: "Ceiling 0.43.2",
      published_at: "2026-07-13T00:00:00Z",
      assets: [
        { name: "Ceiling-0.43.2-Setup.exe", download_count: 14 },
        { name: "Ceiling-0.43.2-portable.exe", download_count: 6 },
        { name: "Ceiling-0.43.2-Setup.exe.sha256", download_count: 80 },
      ],
    },
  ]);

  assert.equal(result.total, 20);
  assert.equal(result.installer, 14);
  assert.equal(result.portable, 6);
  assert.equal(result.latest, 20);
  assert.equal(result.releases[0].assets.length, 2);
});

test("safePath accepts local paths and rejects untrusted values", () => {
  assert.equal(safePath("/download?from=hero"), "/download?from=hero");
  assert.equal(safePath("https://example.com"), "/");
  assert.equal(safePath(null), "/");
});

test("admin routes fail closed and accept a valid token", async () => {
  const env = {
    ADMIN_TOKEN: "test-admin-token",
    ASSETS: {
      fetch: async () => new Response("admin dashboard", { headers: { "Content-Type": "text/html" } }),
    },
  };
  const ctx = { waitUntil() {} };
  const login = await worker.fetch(new Request("https://ceiling.win/admin"), env, ctx);
  assert.equal(login.status, 401);
  assert.match(await login.text(), /Ceiling analytics/);

  const wrong = await worker.fetch(new Request("https://ceiling.win/admin/session", {
    method: "POST",
    headers: { "Content-Type": "application/json", Origin: "https://ceiling.win" },
    body: JSON.stringify({ token: "wrong" }),
  }), env, ctx);
  assert.equal(wrong.status, 401);

  const session = await worker.fetch(new Request("https://ceiling.win/admin/session", {
    method: "POST",
    headers: { "Content-Type": "application/json", Origin: "https://ceiling.win" },
    body: JSON.stringify({ token: "test-admin-token" }),
  }), env, ctx);
  assert.equal(session.status, 200);
  const setCookie = session.headers.get("Set-Cookie");
  assert.match(setCookie, /HttpOnly/);
  assert.match(setCookie, /Secure/);
  assert.match(setCookie, /SameSite=Strict/);
  const cookie = setCookie.split(";")[0];

  const dashboard = await worker.fetch(new Request("https://ceiling.win/admin", {
    headers: { Cookie: cookie },
  }), env, ctx);
  assert.equal(dashboard.status, 200);
  assert.equal(dashboard.headers.get("Cache-Control"), "private, no-store");
  assert.equal(await dashboard.text(), "admin dashboard");
});

test("admin routes reject cross-origin login and direct private assets", async () => {
  const env = { ADMIN_TOKEN: "test-admin-token" };
  const crossOrigin = await worker.fetch(new Request("https://ceiling.win/admin/session", {
    method: "POST",
    headers: { "Content-Type": "application/json", Origin: "https://example.com" },
    body: JSON.stringify({ token: "test-admin-token" }),
  }), env, { waitUntil() {} });
  assert.equal(crossOrigin.status, 401);

  const privateAsset = await worker.fetch(
    new Request("https://ceiling.win/_private/admin-dashboard.txt"),
    env,
    { waitUntil() {} },
  );
  assert.equal(privateAsset.status, 404);
});

test("event capture is a no-op when analytics is not configured", async () => {
  const response = await worker.fetch(new Request("https://ceiling.win/api/events", {
    method: "POST",
    headers: { "Content-Type": "application/json", Origin: "https://ceiling.win" },
    body: JSON.stringify({ event: "$pageview", pathname: "/" }),
  }), {}, { waitUntil() {} });
  assert.equal(response.status, 204);
});
