import { describe, expect, it } from "vitest";
// Imported rather than read from disk: the frontend has no @types/node, and
// importing the real config files is what makes this a regression test instead
// of a copy of the strings.
import shipped from "../../src-tauri/tauri.conf.json";
import devOverlay from "../../src-tauri/tauri.dev.conf.json";
import packageJson from "../../package.json";

type Directives = Map<string, string[]>;

const parseCsp = (csp: string): Directives =>
  new Map(
    csp
      .split(";")
      .map((directive) => directive.trim())
      .filter(Boolean)
      .map((directive) => {
        const [name, ...sources] = directive.split(/\s+/);
        return [name, sources] as [string, string[]];
      }),
  );

// `http://ipc.localhost` is Tauri's Windows IPC origin, not a loopback port, so
// match the host exactly instead of substring-searching for "localhost".
const LOOPBACK_HOSTS = new Set(["localhost", "127.0.0.1", "[::1]"]);

const isLoopbackSource = (source: string) =>
  LOOPBACK_HOSTS.has(
    source
      .replace(/^[a-z]+:\/\//, "")
      .replace(/:[*\d]+$/, ""),
  );

const shippedCsp = parseCsp(shipped.app.security.csp);
const devCsp = parseCsp(devOverlay.app.security.csp);

describe("shipped Tauri CSP", () => {
  it("grants the webview no access to loopback ports", () => {
    expect(shippedCsp.get("connect-src")?.filter(isLoopbackSource)).toEqual([]);
  });

  it("still allows the IPC transports the app needs", () => {
    expect(shippedCsp.get("connect-src")).toEqual(["'self'", "ipc:", "http://ipc.localhost"]);
  });

  it("has no loopback source in any other directive", () => {
    for (const [name, sources] of shippedCsp) {
      expect({ [name]: sources.filter(isLoopbackSource) }).toEqual({ [name]: [] });
    }
  });
});

describe("dev CSP overlay", () => {
  it("is wired into the tauri:dev script", () => {
    expect(packageJson.scripts["tauri:dev"]).toContain("--config src-tauri/tauri.dev.conf.json");
  });

  it("re-opens loopback so Vite and its HMR socket work", () => {
    expect(devCsp.get("connect-src")?.filter(isLoopbackSource)).toEqual([
      "http://localhost:*",
      "http://127.0.0.1:*",
      "ws://localhost:*",
      "ws://127.0.0.1:*",
    ]);
  });

  it("differs from the shipped CSP only in connect-src", () => {
    const withoutConnectSrc = (directives: Directives) =>
      Object.fromEntries([...directives].filter(([name]) => name !== "connect-src"));

    expect(withoutConnectSrc(devCsp)).toEqual(withoutConnectSrc(shippedCsp));
  });
});
