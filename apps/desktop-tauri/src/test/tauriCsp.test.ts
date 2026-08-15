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

/**
 * Host of a CSP source, lowercased, or the source unchanged when it has no
 * host.
 *
 * Everything after the host is dropped, not just a port: trimming only a
 * trailing `:port` left `http://localhost:1420/` reading as the host
 * `localhost:1420/`, which is in no set.
 *
 * Schemes and hosts are case-insensitive in CSP, and a scheme may contain
 * `+`, `-`, and `.` (RFC 3986), so `HTTP://LOCALHOST:1420` and
 * `custom+scheme://127.0.0.1` are loopback too. Every miss here quietly
 * disarms the checks below, so the parsing is deliberately permissive about
 * how a source is spelled.
 */
const sourceHost = (source: string): string => {
  const withoutScheme = source.toLowerCase().replace(/^[a-z][a-z0-9+.-]*:\/\//, "");
  // A bracketed IPv6 literal is full of colons, so take the bracket group whole.
  const bracketed = /^\[[^\]]*\]/.exec(withoutScheme);
  return bracketed ? bracketed[0] : withoutScheme.replace(/[:/?#].*$/, "");
};

const isLoopbackSource = (source: string) => LOOPBACK_HOSTS.has(sourceHost(source));

const shippedCsp = parseCsp(shipped.app.security.csp);
const devCsp = parseCsp(devOverlay.app.security.csp);

describe("loopback detection", () => {
  // The detector is the whole gate, so a hole in it silently disarms every
  // check below. A trailing path used to defeat it.
  it("sees loopback however the source is written", () => {
    for (const source of [
      "http://localhost:*",
      "http://localhost:1420/",
      "http://127.0.0.1:3000/api",
      "ws://localhost",
      "http://[::1]:8080",
      "127.0.0.1",
      // Schemes and hosts are case-insensitive in CSP.
      "HTTP://localhost:1420",
      "http://LOCALHOST:1420",
      // A scheme may carry +, -, and . (RFC 3986).
      "custom+scheme://127.0.0.1",
      "my-app.v2://localhost",
    ]) {
      expect({ [source]: isLoopbackSource(source) }).toEqual({ [source]: true });
    }
  });

  it("leaves the IPC origin and keywords alone", () => {
    for (const source of ["'self'", "ipc:", "http://ipc.localhost", "https://api.openai.com"]) {
      expect({ [source]: isLoopbackSource(source) }).toEqual({ [source]: false });
    }
  });
});

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

  // Dev has to be a superset. If the shipped CSP later needs a real origin,
  // adding it there alone leaves dev unable to reach it, and the two checks
  // above would both still pass: one only inspects loopback, the other only
  // compares the directives that are not connect-src.
  it("offers everything the shipped CSP allows", () => {
    const devSources = new Set(devCsp.get("connect-src") ?? []);
    const missing = (shippedCsp.get("connect-src") ?? []).filter(
      (source) => !devSources.has(source),
    );
    expect(missing).toEqual([]);
  });

  it("differs from the shipped CSP only in connect-src", () => {
    const withoutConnectSrc = (directives: Directives) =>
      Object.fromEntries([...directives].filter(([name]) => name !== "connect-src"));

    expect(withoutConnectSrc(devCsp)).toEqual(withoutConnectSrc(shippedCsp));
  });
});
