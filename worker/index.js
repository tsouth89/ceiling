// Cloudflare Worker for ceiling.win.
//
// Serves the static site from the ASSETS binding, and adds two evergreen
// download routes that resolve to the newest signed installer at click time,
// so the marketing page never has to be edited when a release ships:
//
//   /download           -> latest *-Setup.exe
//   /download/portable   -> latest *-portable.exe
//
// The GitHub "latest release" lookup is cached at the edge for five minutes,
// and any failure falls back to the GitHub releases page.

const REPO = "tsouth89/ceiling";
const LATEST_PAGE = `https://github.com/${REPO}/releases/latest`;

const DOWNLOAD_ROUTES = {
  "/download": /-Setup\.exe$/i,
  "/download/portable": /-portable\.exe$/i,
};

export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url);
    const pathname = url.pathname.replace(/\/+$/, "") || "/";
    const assetPattern = DOWNLOAD_ROUTES[pathname];

    if (assetPattern) {
      const target = await latestAssetUrl(assetPattern, ctx).catch(() => LATEST_PAGE);
      return Response.redirect(target, 302);
    }

    return env.ASSETS.fetch(request);
  },
};

async function latestAssetUrl(pattern, ctx) {
  const cache = caches.default;
  const cacheKey = new Request("https://ceiling.win/__latest_release_v1");

  let cached = await cache.match(cacheKey);
  let data;
  if (cached) {
    data = await cached.json();
  } else {
    const res = await fetch(`https://api.github.com/repos/${REPO}/releases/latest`, {
      headers: {
        "User-Agent": "ceiling-site",
        Accept: "application/vnd.github+json",
      },
      signal: AbortSignal.timeout(3000),
    });
    if (!res.ok) throw new Error(`github api ${res.status}`);
    const body = await res.text();
    const store = new Response(body, {
      headers: {
        "Content-Type": "application/json",
        "Cache-Control": "public, max-age=300",
      },
    });
    ctx.waitUntil(cache.put(cacheKey, store.clone()));
    data = JSON.parse(body);
  }

  const asset = (data.assets || []).find((a) => pattern.test(a.name));
  if (!asset) throw new Error("no matching asset");
  return asset.browser_download_url;
}
