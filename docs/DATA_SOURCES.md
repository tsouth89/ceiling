# How Ceiling gets your data

Ceiling is a local-first Windows app. It reads your AI usage from sources that
already live on your PC, or by calling each provider's own usage endpoint.
**Your provider credentials and usage data are stored only on this PC, and
Ceiling never sends them to a Ceiling-operated server.** Fetching usage does
make network requests, but only to the provider you enabled (and, for Wayfinder,
to the local gateway URL you configure yourself), never to Ceiling's own
servers.

This document is the maintained reference behind the "How Ceiling gets this
data" panel in a provider's Settings detail view.

## What Ceiling stores locally

| Data | Where | Protection |
|------|-------|------------|
| API keys | `%APPDATA%\Ceiling` (ApiKeys store) | Windows DPAPI, user-only file ACL |
| Manual cookies | `%APPDATA%\Ceiling` (ManualCookies store) | Windows DPAPI, user-only file ACL |
| OAuth token accounts | `%APPDATA%\Ceiling` (token store) | Windows DPAPI, user-only file ACL |
| Settings | `%APPDATA%\Ceiling\settings.json` | Local file |

Ceiling also *reads* credentials that other tools already wrote (for example
`~/.codex/auth.json`, the Cursor IDE session database, or your browser's cookie
database). It does not copy those into its own storage unless you explicitly
import them. See [COOKIES.md](COOKIES.md) for browser cookie extraction and
App-Bound Encryption details.

## Per-provider sources

| Provider | How Ceiling reads usage | Talks to |
|----------|-------------------------|----------|
| **Claude** | Your signed-in Claude Code / Claude Desktop session, an OAuth token, or claude.ai cookies you provide; plus local `~/.claude` logs for cost and tokens | api.anthropic.com, claude.ai |
| **Codex / OpenAI** | The OAuth token from your Codex CLI (`~/.codex/auth.json`) plus local `~/.codex/sessions` logs | chatgpt.com |
| **Cursor** | Your signed-in Cursor IDE session (`state.vscdb`), or cursor.com cookies you provide | cursor.com |
| **GitHub Copilot** | Your GitHub CLI / Git Credential Manager token (or a device-flow sign-in) | api.github.com, or your GitHub Enterprise host |
| **Gemini** | The OAuth token from the Gemini CLI (`~/.gemini/oauth_creds.json`) | Google Code Assist and OAuth endpoints |
| Other providers | A local session or token on this PC, or the provider's own usage endpoint | The provider's own domain only |

## Network policy

- The Ceiling desktop app makes no analytics or telemetry calls to
  Ceiling-operated servers.
- Every usage request targets the provider's own domain listed above.
- The only non-provider destination is **Wayfinder**, which calls the local
  gateway URL you configure yourself.

If you enable a provider and don't see it described here or in the in-app panel,
that's a bug. Please open an issue.
