# codexbar CLI

| | |
|---|---|
| **Status** | Available |
| **Location** | docs/CLI.md |
| **Binary** | `codexbar` |

`codexbar` is the CLI-only companion to the Ceiling desktop app. It inspects
provider usage and cost, exports diagnostics, exposes local endpoints, and
manages configuration and token accounts.

Build or run the binary from the repository root:

```powershell
cargo run --manifest-path rust/Cargo.toml -- --help
```

## Global flags

These flags are accepted by the top-level command and by subcommands.

| Flag | Description |
|---|---|
| `-v, --verbose` | Enable verbose logging. |
| `--json-output` | Emit machine-readable logs (JSON) to stderr. |
| `--log-level <LEVEL>` | `trace`, `debug`, `info`, `warn`, or `error`. |
| `--no-color` | Disable ANSI colors in output. |
| `-h, --help` | Print help. |
| `-V, --version` | Print version. |

## `usage`

Print usage from enabled providers as text or JSON. This is the default
command when no subcommand is provided.

```text
codexbar usage [OPTIONS]
```

Useful options:

- `-p, --provider <PROVIDER>` - provider CLI name or alias.
- `-f, --format <text|json>` - output format.
- `--json` - shorthand for `--format json`.
- `--status` - include provider status pages.
- `--all-accounts` - fetch all token accounts where supported.
- `--source <auto|web|cli|oauth>` - data source.
- `--brief` - one compact line per provider.

Examples:

```sh
codexbar usage
codexbar usage --provider codex --json --pretty
codexbar --provider all --brief
```

## `cost`

Print local token cost usage for Claude, Codex, and Grok without web or CLI access.

```text
codexbar cost [OPTIONS]
```

Useful options:

- `-p, --provider <PROVIDER>` - provider to query.
- `-d, --days <DAYS>` - number of days to scan (default `30`).
- `--codex-speed <auto|standard|fast>` - Codex cost speed tier.
- `--json` / `--pretty` - JSON output.

Example:

```sh
codexbar cost --days 7 --json --pretty
```

## `diagnose`

Export safe provider diagnostics as JSON. Identity values, credentials,
provider response bodies, and raw provider error text are excluded; failures
use normalized local categories and messages.

```text
codexbar diagnose [OPTIONS]
```

Useful options:

- `-p, --provider <PROVIDER>` - provider to diagnose, or `all` (default).
- `--source <auto|web|cli|oauth>` - data source.
- `--pretty` - pretty-print JSON output.

Example:

```sh
codexbar diagnose --provider claude --pretty
```

## `sessions`

List or focus local and configured remote agent sessions.

```text
codexbar sessions [OPTIONS]
```

Useful options:

- `--json` - emit machine-readable session data.
- `--brief` - one compact line per session.
- `--ssh-host <HOST>` - discover sessions on a configured SSH host.
- `--focus <ID>` - focus one session by its stable id.

Example:

```sh
codexbar sessions --json --pretty
```

## `serve`

Serve usage and cost JSON on `127.0.0.1`.

```text
codexbar serve [OPTIONS]
```

Useful options:

- `--port <PORT>` - local HTTP port (default `8080`).
- `--refresh-interval <SECONDS>` - response cache TTL (default `60`).
- `--allow-unauthenticated` - skip the per-user bearer token. Any local process can then read usage. Existing scripts that do not send `Authorization` need this flag.
- `--include-identity` - include account email, organization, login method, and raw provider errors. Those fields are omitted by default.

On first start, Ceiling prints an `Authorization: Bearer …` token and stores it at `<user config dir>/Ceiling/serve.token` (current-user ACL on Windows, mode `0600` elsewhere). Later starts print only the path. `/health` stays unauthenticated. `/usage` and `/cost` require the token unless `--allow-unauthenticated` is set.

Example:

```sh
codexbar serve --port 8080
# Copy the token from the first-start output, or read the stored file:
TOKEN="$(cat "${XDG_CONFIG_HOME:-$HOME/.config}/Ceiling/serve.token")"
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:8080/usage
```

## `statusline`

Print one compact usage line for an editor status bar. Cache-only.

```text
codexbar statusline [OPTIONS]
```

Useful options:

- `-p, --provider <PROVIDER>` - provider to show.
- `--no-cost` - hide the estimated-spend segment.

Example:

```sh
codexbar statusline --provider codex --no-cost
```

## `mcp`

Expose usage and spend over MCP stdio. Local-first, no network.

Tools: `get_usage` reads the desktop widget snapshot (cache-only). `get_status` is the cheap cap check — snapshot quota plus today's estimated spend from a 1-day local log scan, not a 30-day rescan. `get_spend` walks local Codex/Claude/Grok logs for today, 7 days, and 30 days.

```text
codexbar mcp [OPTIONS]
```

Useful options:

- `--include-identity` - include account email and login method in tool output. Those fields are omitted by default, like `serve`.

Tools:

| Tool | Source | Notes |
|---|---|---|
| `list_providers` | widget snapshot + settings | Quota cache presence and whether local spend scanning is supported. |
| `get_usage` | widget snapshot | Remaining quota windows. `period_cost_usd` is the provider's billed / current-period `CostSnapshot.used`, with `cost_period` as the provider's period label (for example `Monthly`). It is **not** this conversation's spend. |
| `get_spend` | local Codex / Claude / Grok logs | Estimated API-value spend for today, 7 days, and 30 days. Not a bill. |
| `get_status` | snapshot + local logs | Compact remaining-quota plus `today_spend`. `remaining_percent` is the constraining window across primary/secondary/tertiary (exhausted first, then highest used %), not `usage.primary` alone. `usage` is the same object as `get_usage` (including `period_cost_usd`). `today_spend` is local estimated log spend for today. |

`session_cost_usd` is not emitted. Older builds stuffed billed period cost into that name.

Example:

```sh
codexbar mcp
```

## `autostart`

Manage auto-start on Windows boot.

```text
codexbar autostart [OPTIONS]
```

Useful options:

- `--enable` - enable auto-start.
- `--disable` - disable auto-start.
- `--status` - show current status.

Examples:

```sh
codexbar autostart --status
codexbar autostart --enable
```

## `account`

Manage token accounts for providers.

```text
codexbar account <COMMAND>
```

| Subcommand | Description |
|---|---|
| `list <PROVIDER>` | List accounts for a provider. |
| `add <PROVIDER> --label <LABEL> --token <TOKEN>` | Add a new account. |
| `remove <PROVIDER> <ACCOUNT>` | Remove an account. |
| `switch <PROVIDER> <ACCOUNT>` | Switch active account. |

Examples:

```sh
codexbar account list claude
codexbar account add claude --label Personal --token <token>
codexbar account switch claude Personal
```

## `config`

Configuration utilities.

```text
codexbar config <COMMAND>
```

| Subcommand | Description |
|---|---|
| `validate` | Validate configuration files. |
| `dump` | Dump configuration to stdout (`--format json` or `toml`). |
| `providers` | List providers and enabled state. |
| `enable <PROVIDER>` | Enable a provider. |
| `disable <PROVIDER>` | Disable a provider. |
| `set-api-key <PROVIDER>` | Store an API key for a provider. |
| `path` | Show configuration file paths. |

Examples:

```sh
codexbar config validate
codexbar config providers
codexbar config enable codex
codexbar config set-api-key --stdin codex
```

## See also

- [DATA_SOURCES.md](DATA_SOURCES.md)
- [SETTINGS_JSON.md](SETTINGS_JSON.md)
