# Ceiling site analytics

Ceiling's Cloudflare Worker serves the static site, records a small set of first-party events, and protects the private dashboard at `https://ceiling.win/admin`.

## What the dashboard measures

- Website visitors, pageviews, download clicks, paths, and referrers for the trailing 30 days through PostHog.
- Cumulative GitHub release asset downloads, split between installer and portable builds.
- Repository stars, forks, and open issues.
- GitHub repository visitors, clones, referrers, and popular paths for GitHub's trailing 14-day window.

GitHub release downloads are download events, not unique users. GitHub does not expose historical repository traffic beyond its rolling 14-day window.

## Privacy

The landing page does not load a third-party analytics script or set analytics cookies. It sends pageview and link-click events to Ceiling's own Worker. The Worker creates an anonymous visitor identifier by hashing the request IP, user agent, and a private salt before sending the event to PostHog. Raw IP addresses and user agents are not included in the analytics event.

## Cloudflare configuration

Set these Worker secrets:

```powershell
npx wrangler secret put ADMIN_TOKEN
npx wrangler secret put ANALYTICS_SALT
npx wrangler secret put POSTHOG_PUBLIC_KEY
npx wrangler secret put POSTHOG_QUERY_KEY
npx wrangler secret put GITHUB_TOKEN
```

Set these Worker variables in the Cloudflare dashboard, or add non-secret values to the production environment configuration:

| Variable | Purpose |
| --- | --- |
| `POSTHOG_PROJECT_ID` | Numeric ID of the Ceiling PostHog project. |
| `POSTHOG_QUERY_HOST` | Optional. Defaults to `https://us.posthog.com`. |
| `POSTHOG_CAPTURE_HOST` | Optional. Defaults to `https://us.i.posthog.com`. |
| `GITHUB_REPO` | Optional. Defaults to `tsouth89/ceiling`. |

Use a separate PostHog project for Ceiling. `POSTHOG_PUBLIC_KEY` is that project's `phc_...` project key. `POSTHOG_QUERY_KEY` is a personal `phx_...` API key with Query Read access to the project.

For `GITHUB_TOKEN`, use a fine-grained token limited to the Ceiling repository with **Administration: Read** permission. GitHub's public API does not need this token for release counts, but the protected Traffic API does.

Generate long random values for `ADMIN_TOKEN` and `ANALYTICS_SALT`; do not commit either value. If PostHog is not configured, GitHub release and repository totals still work and the dashboard shows a setup note in place of website traffic. If the GitHub token is absent, public repository and download totals still work while the 14-day traffic panel remains disabled.

## Local validation

Create a `.dev.vars` file that is excluded from git:

```text
ADMIN_TOKEN=local-test-token
ANALYTICS_SALT=local-test-salt
POSTHOG_PUBLIC_KEY=phc_example
POSTHOG_QUERY_KEY=phx_example
POSTHOG_PROJECT_ID=12345
GITHUB_TOKEN=github_pat_example
```

Then run:

```powershell
npx wrangler dev
```

Open `http://localhost:8787/admin`. Never commit `.dev.vars`.
