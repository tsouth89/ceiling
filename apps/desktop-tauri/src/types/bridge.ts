export type SurfaceMode = "hidden" | "trayPanel" | "popOut" | "settings";
export type VisibleSurfaceMode = Exclude<SurfaceMode, "hidden">;
export type SettingsTabId =
  | "general"
  | "providers"
  | "notifications"
  | "menuBar"
  | "menu"
  | "advanced"
  | "about";

// ── Narrowed string-literal unions (persisted settings enums) ─────────

export type TrayIconMode = "single" | "perProvider";

export type MetricPreference =
  | "automatic"
  | "session"
  | "weekly"
  | "model"
  | "tertiary"
  | "credits"
  | "extraUsage"
  | "average";

export type Language =
  | "english"
  | "chinese"
  | "chinesetraditional"
  | "japanese"
  | "korean"
  | "spanish";

/** Language catalog entry from the Rust backend. */
export type LanguageOption = {
  /** Stable bridge/settings value (e.g. "english") */
  value: Language;
  /** Native display name (e.g. "English", "中文", "Español") */
  display: string;
};

export type UpdateChannel = "stable" | "beta";

export type ThemePreference = "auto" | "light" | "dark";

export type MenuBarDisplayMode = "minimal" | "compact" | "detailed";
export type FloatBarOrientation = "horizontal" | "vertical";
export type FloatBarStyle = "floating" | "taskbar";
export type FloatBarDensity = "compact" | "standard" | "detailed";
export type FloatBarInformationMode = "exact" | "calm";
export type FloatBarContrast = "auto" | "light-text" | "dark-text";
export type ProofProviderId =
  | "codex"
  | "claude"
  | "cursor"
  | "factory"
  | "gemini"
  | "antigravity"
  | "copilot"
  | "zai"
  | "minimax"
  | "kiro"
  | "vertexai"
  | "augment"
  | "opencode"
  | "kimi"
  | "kimik2"
  | "amp"
  | "warp"
  | "ollama"
  | "azureopenai"
  | "t3chat"
  | "openrouter"
  | "jetbrains"
  | "alibaba"
  | "alibabatokenplan"
  | "nanogpt"
  | "infini"
  | "perplexity"
  | "abacus"
  | "opencodego"
  | "kilo"
  | "bedrock"
  | "mistral"
  | "codebuff"
  | "deepseek"
  | "windsurf"
  | "manus"
  | "mimo"
  | "doubao"
  | "commandcode"
  | "crof"
  | "stepfun"
  | "venice"
  | "openaiapi"
  | "grok"
  | "elevenlabs"
  | "deepgram"
  | "groq"
  | "llmproxy"
  | "chutes"
  | "litellm"
  | "poe"
  | "devin"
  | "zed"
  | "crossmodel"
  | "qoder"
  | "sakana"
  | "wayfinder";

export type TrayPanelSurfaceTarget = { kind: "summary" };
export type PopOutSurfaceTarget =
  | { kind: "dashboard" }
  | { kind: "provider"; providerId: string };
export type SettingsSurfaceTarget = { kind: "settings"; tab: SettingsTabId };

export type SurfaceTarget =
  | TrayPanelSurfaceTarget
  | PopOutSurfaceTarget
  | SettingsSurfaceTarget;

export type SurfaceTargetForMode<M extends VisibleSurfaceMode> =
  M extends "trayPanel"
    ? TrayPanelSurfaceTarget
    : M extends "popOut"
      ? PopOutSurfaceTarget
      : SettingsSurfaceTarget;

export interface CurrentSurfaceState {
  mode: SurfaceMode;
  target: SurfaceTarget;
}

export interface AgentSession {
  id: string;
  provider: "codex" | "claude";
  source: "cli" | "desktopApp" | "ide" | "unknown";
  state: "active" | "idle";
  pid: number | null;
  transcriptPath: string | null;
  host: string;
  workspace: {
    cwd: string | null;
    projectName: string | null;
  };
  activity: {
    startedAt: string | null;
    lastActivityAt: string | null;
  };
  focusTarget:
    | { kind: "process"; pid: number }
    | { kind: "transcript"; transcriptPath: string }
    | { kind: "none" };
}

export interface AgentSessionHostResult {
  host: string;
  sessions: AgentSession[];
  error: string | null;
}

export type AgentSessionDiscoveryResult =
  | { status: "disabled" }
  | { status: "hosts"; hosts: AgentSessionHostResult[] };

export type SessionFocusResult =
  | { status: "focused" }
  | { status: "unsupported"; message: string }
  | { status: "failed"; message: string };

export interface ProofRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface ProofStatePayload {
  mode: SurfaceMode;
  target: SurfaceTarget;
  windowRect: ProofRect | null;
  trayAnchor: ProofRect | null;
  workArea: ProofRect | null;
  menuPath: string | null;
  menuItems: string[];
}

export type ProofCommand =
  | "open-tray-panel"
  | "open-native-menu"
  | "open-dashboard"
  | "open-about-path"
  | "hide-surface"
  | `open-provider:${ProofProviderId}`
  | `open-settings:${SettingsTabId}`;

export interface ProviderCatalogEntry {
  id: string;
  displayName: string;
  cookieDomain: string | null;
}

export interface ProviderSummary {
  id: string;
  displayName: string;
  enabled: boolean;
  order: number;
}

export interface SettingsSnapshot {
  enabledProviders: string[];
  providerOrder?: string[];
  refreshIntervalSecs: number;
  refreshAllProvidersOnMenuOpen: boolean;
  startAtLogin: boolean;
  startMinimized: boolean;
  showNotifications: boolean;
  capacityEventNotificationsEnabled: boolean;
  soundEnabled: boolean;
  soundVolume: number;
  highUsageThreshold: number;
  criticalUsageThreshold: number;
  /** Optional while reading snapshots from a backend before budget alerts shipped. */
  spendBudgetAlertsEnabled?: boolean;
  /** "daily" or calendar-month-to-date "monthly". */
  spendBudgetPeriod?: "daily" | "monthly";
  spendBudgetWarningUsd?: number;
  spendBudgetLimitUsd?: number;
  providerUsageThresholds?: Record<string, UsageThresholdOverride>;
  predictivePaceWarningEnabled: boolean;
  trayIconMode: TrayIconMode;
  switcherShowsIcons: boolean;
  menuBarShowsHighestUsage: boolean;
  menuBarShowsPercent: boolean;
  showAsUsed: boolean;
  showAllTokenAccountsInMenu: boolean;
  enableAnimations: boolean;
  resetTimeRelative: boolean;
  showResetWhenExhausted: boolean;
  menuBarDisplayMode: MenuBarDisplayMode;
  hidePersonalInfo: boolean;
  updateChannel: UpdateChannel;
  autoDownloadUpdates: boolean;
  installUpdatesOnQuit: boolean;
  globalShortcut: string;
  /** Optional while reading settings saved before the taskbar shortcut existed. */
  taskbarToggleShortcut?: string;
  /** Extra Codex home or sessions directories scanned for local usage history. */
  codexCustomSessionsDirs: string[];
  agentSessionsEnabled?: boolean;
  agentSessionSshHosts?: string[];
  uiLanguage: Language;
  theme: ThemePreference;
  /** 100..=250 — clamped server-side. */
  windowScalePercent: number;
  /** 100..=200 — clamped server-side. */
  trayScalePercent: number;
  powertoysStatusPipeEnabled: boolean;
  claudeAvoidKeychainPrompts: boolean;
  codexSparkUsageVisible: boolean;
  disableKeychainAccess: boolean;
  wayfinderGatewayUrl?: string;
  providerMetrics: Record<string, MetricPreference>;
  floatBarEnabled: boolean;
  taskbarWidgetEnabled: boolean;
  taskbarWidgetAllMonitors: boolean;
  /** 30..=100 — clamped server-side. */
  floatBarOpacity: number;
  /** 75..=200 — clamped server-side. */
  floatBarScale: number;
  floatBarOrientation: FloatBarOrientation;
  floatBarStyle: FloatBarStyle;
  /** Open the taskbar glance panel after a short pointer dwell. */
  taskbarWidgetOpenOnHover: boolean;
  floatBarDensity: FloatBarDensity;
  /** "exact" (icon + %) or "calm" (pace state + next reset). Separate from density. */
  floatBarInformationMode: FloatBarInformationMode;
  floatBarContrast: FloatBarContrast;
  floatBarClickThrough: boolean;
  /** Empty array = show all enabled providers. */
  floatBarProviderIds: string[];
  /** When true, render with dark text/glass for light desktops. */
  floatBarDarkText: boolean;
  /** When true, render the next primary reset inline in each provider pill. */
  floatBarShowResetInline: boolean;
  /** Legacy compatibility field; API-equivalent cost pills are no longer rendered. */
  floatBarShowCost: boolean;
}

/** Partial settings object — only include fields you want to change. */
export interface SettingsUpdate {
  enabledProviders?: string[];
  refreshIntervalSecs?: number;
  refreshAllProvidersOnMenuOpen?: boolean;
  startAtLogin?: boolean;
  startMinimized?: boolean;
  showNotifications?: boolean;
  capacityEventNotificationsEnabled?: boolean;
  soundEnabled?: boolean;
  soundVolume?: number;
  highUsageThreshold?: number;
  criticalUsageThreshold?: number;
  spendBudgetAlertsEnabled?: boolean;
  spendBudgetPeriod?: "daily" | "monthly";
  spendBudgetWarningUsd?: number;
  spendBudgetLimitUsd?: number;
  providerUsageThresholds?: Record<string, UsageThresholdOverride>;
  predictivePaceWarningEnabled?: boolean;
  trayIconMode?: TrayIconMode;
  switcherShowsIcons?: boolean;
  menuBarShowsHighestUsage?: boolean;
  menuBarShowsPercent?: boolean;
  showAsUsed?: boolean;
  showAllTokenAccountsInMenu?: boolean;
  enableAnimations?: boolean;
  resetTimeRelative?: boolean;
  showResetWhenExhausted?: boolean;
  menuBarDisplayMode?: MenuBarDisplayMode;
  hidePersonalInfo?: boolean;
  updateChannel?: UpdateChannel;
  autoDownloadUpdates?: boolean;
  installUpdatesOnQuit?: boolean;
  globalShortcut?: string;
  taskbarToggleShortcut?: string;
  codexCustomSessionsDirs?: string[];
  agentSessionsEnabled?: boolean;
  agentSessionSshHosts?: string[];
  uiLanguage?: Language;
  theme?: ThemePreference;
  windowScalePercent?: number;
  trayScalePercent?: number;
  powertoysStatusPipeEnabled?: boolean;
  claudeAvoidKeychainPrompts?: boolean;
  codexSparkUsageVisible?: boolean;
  disableKeychainAccess?: boolean;
  /** Map of provider CLI name → metric preference label. */
  providerMetrics?: Record<string, MetricPreference>;
  floatBarEnabled?: boolean;
  taskbarWidgetEnabled?: boolean;
  taskbarWidgetAllMonitors?: boolean;
  floatBarOpacity?: number;
  floatBarScale?: number;
  floatBarOrientation?: FloatBarOrientation;
  floatBarStyle?: FloatBarStyle;
  taskbarWidgetOpenOnHover?: boolean;
  floatBarDensity?: FloatBarDensity;
  floatBarInformationMode?: FloatBarInformationMode;
  floatBarContrast?: FloatBarContrast;
  floatBarClickThrough?: boolean;
  floatBarProviderIds?: string[];
  floatBarDarkText?: boolean;
  floatBarShowResetInline?: boolean;
  floatBarShowCost?: boolean;
}

export interface UsageThresholdOverride {
  high?: number;
  critical?: number;
}

export interface BootstrapState {
  contractVersion: string;
  providers: ProviderCatalogEntry[];
  settings: SettingsSnapshot;
}

export type DetectedAccountStatus =
  | "ready"
  | "locked"
  | "installed"
  | "unavailable";

export interface DetectedProviderAccount {
  providerId: string;
  displayName: string;
  status: DetectedAccountStatus;
  sourceLabel: string;
  detail: string;
}

// ── Provider usage snapshot types ────────────────────────────────────

export interface RateWindowSnapshot {
  usedPercent: number;
  remainingPercent: number;
  windowMinutes: number | null;
  resetsAt: string | null;
  resetDescription: string | null;
  isExhausted: boolean;
  reservePercent: number | null;
  reserveDescription: string | null;
  reserveWillLastToReset?: boolean;
  reserveEtaSeconds?: number | null;
}

export interface CostSnapshotBridge {
  used: number;
  limit: number | null;
  remaining: number | null;
  currencyCode: string;
  period: string;
  resetsAt: string | null;
  formattedUsed: string;
  formattedLimit: string | null;
}

export interface PaceSnapshot {
  windowLabel: string;
  stage: "on_track" | "slightly_ahead" | "ahead" | "far_ahead" | "slightly_behind" | "behind" | "far_behind";
  deltaPercent: number;
  willLastToReset: boolean;
  etaSeconds: number | null;
  expectedUsedPercent: number;
  actualUsedPercent: number;
}

export interface ProviderUsageSnapshot {
  providerId: string;
  displayName: string;
  primary: RateWindowSnapshot;
  primaryLabel?: string;
  secondary: RateWindowSnapshot | null;
  secondaryLabel?: string;
  modelSpecific: RateWindowSnapshot | null;
  tertiary: RateWindowSnapshot | null;
  extraRateWindows: Array<{
    id: string;
    title: string;
    window: RateWindowSnapshot;
  }>;
  inactiveRateWindows?: Array<{
    id: string;
    title: string;
    description: string;
    /**
     * "notEnforced": the provider reported no active limit for this window.
     * "unavailable": a tracked window dropped out of an otherwise-successful
     * response. Optional for back-compat; treat a missing value as notEnforced.
     */
    state?: "notEnforced" | "unavailable";
  }>;
  promoSignals?: Array<{
    id: string;
    kind: "boost" | "inclusion";
    title: string;
    description: string;
    windowId?: string | null;
    endsAt?: string | null;
  }>;
  resetCreditsAvailable?: number | null;
  cost: CostSnapshotBridge | null;
  planName: string | null;
  accountEmail: string | null;
  sourceLabel: string;
  updatedAt: string;
  error: string | null;
  pace: PaceSnapshot | null;
  accountOrganization: string | null;
  trayStatusLabel: string | null;
  fetchDurationMs?: number | null;
  wayfinderUsage?: WayfinderUsageSnapshot | null;
}

export interface WayfinderRouteSummary {
  name: string;
  requests: number;
  tokens: number;
  realized: number;
  baseline: number;
  saved: number;
}

export interface WayfinderUsageSnapshot {
  gatewayStatus: string;
  offline: boolean;
  dryRun: boolean;
  missingKeys: string[];
  modelCount: number;
  models: string[];
  requests: number;
  estimatedRequests: number;
  tokens: number;
  realized: number;
  baseline: number;
  saved: number;
  savedPercent: number;
  periodDays: number;
  unit: string;
  priced: boolean;
  routes: WayfinderRouteSummary[];
}

export interface RefreshCompletePayload {
  providerCount: number;
  errorCount: number;
}

export interface RefreshStartedPayload {
  providerIds: string[];
}

export interface CapacityEventPayload {
  providerId: string;
  displayName: string;
  windowId: string;
  windowLabel: string;
  kind:
    | "scheduledReset"
    | "surpriseReset"
    | "partialReset"
    | "bankedResetGranted"
    | "resetTimeShift"
    | "windowLifted"
    | "windowRestored"
    | "allowanceGranted";
  previousUsedPercent: number;
  currentUsedPercent: number;
  previousResetCredits?: number;
  currentResetCredits?: number;
  previousResetAt: string;
  currentResetAt: string;
  occurredAt: string;
}

export interface SafeDiagnostics {
  appVersion: string;
  platform: string;
  enabledProviders: string[];
  providerCookieSources: Record<string, string>;
  hasManualCookies: string[];
  hasApiKeys: string[];
  hidePersonalInfo: boolean;
  refreshIntervalSecs: number;
}

export interface CredentialStorageStatus {
  manualCookies: string;
  apiKeys: string;
  tokenAccounts: string;
}

// ── Update state types ───────────────────────────────────────────────

export type UpdateStatus =
  | "idle"
  | "checking"
  | "available"
  | "downloading"
  | "ready"
  | "error";

export interface UpdateStatePayload {
  status: UpdateStatus;
  version: string | null;
  error: string | null;
  progress: number | null;
  releaseUrl: string | null;
  canDownload: boolean;
  canApply: boolean;
  /** Unix-ms timestamp of the last completed update check, or `null`
   *  if the app has not checked during this session. */
  lastCheckedAt: number | null;
}

// ── Credential store types ───────────────────────────────────────────

export interface ApiKeyInfoBridge {
  providerId: string;
  provider: string;
  maskedKey: string;
  savedAt: string;
  label: string | null;
}

export interface ApiKeyProviderInfoBridge {
  id: string;
  displayName: string;
  envVar: string | null;
  help: string | null;
  dashboardUrl: string | null;
}

export interface CookieInfoBridge {
  providerId: string;
  provider: string;
  savedAt: string;
}

export interface DetectedBrowserBridge {
  browserType: string;
  displayName: string;
  profileCount: number;
}

export interface AppInfoBridge {
  name: string;
  version: string;
  buildNumber: string;
  updateChannel: string;
  tagline: string;
}

// ── Chart data types ─────────────────────────────────────────────────

export interface DailyCostPoint {
  date: string;
  value: number;
}

export interface ServiceUsagePoint {
  service: string;
  creditsUsed: number;
}

export interface DailyUsageBreakdown {
  day: string;
  services: ServiceUsagePoint[];
  totalCreditsUsed: number;
}

export interface ProviderLocalUsageSummary {
  todayCost: number | null;
  lastSessionCost: number | null;
  lastSessionTokens: number | null;
  lastSessionTokenBreakdown?: LocalTokenBreakdown | null;
  sevenDayCost: number | null;
  sevenDayTokens: number | null;
  sevenDayTokenBreakdown?: LocalTokenBreakdown | null;
  thirtyDayCost: number | null;
  thirtyDayTokens: number | null;
  thirtyDayTokenBreakdown?: LocalTokenBreakdown | null;
  currentWindows: LocalUsageWindowSummary[];
  comparisonPeriods: LocalUsageComparisonPeriod[];
  latestTokens: number | null;
  topModel: string | null;
  modelBreakdown?: LocalModelCost[];
  effortBreakdown?: LocalEffortCost[];
  /** Plans seen in local logs. More than one means the total is not
   *  account-scoped; it spans several plans, not necessarily several accounts. */
  planBreakdown?: LocalPlanUsage[];
  projectBreakdown?: LocalProjectCost[];
  estimateNote: string;
  tokenCostUpdatedAtMs: number;
}

export interface LocalModelCost {
  model: string;
  /** Dollar cost for the period, or null when the model has no canonical price. */
  cost: number | null;
  tokens: number;
  /** Cache-read share of processed tokens (0–100), when any tokens exist. */
  cacheReadPercent?: number | null;
  /** Estimated USD per usage record, when cost and calls are both present. */
  costPerCall?: number | null;
  /** Output tokens per usage record, when calls > 0. */
  outputTokensPerCall?: number | null;
  calls?: number;
}

/** Locally observed activity attributed to one subscription plan. */
export interface LocalPlanUsage {
  plan: string;
  tokens: number;
}

export interface LocalEffortCost {
  /** Reasoning-effort tier (Codex only): "high" / "xhigh" / "medium" / "unknown". */
  effort: string;
  /** Dollar cost for the tier, or null when its models have no canonical price. */
  cost: number | null;
  tokens: number;
}

export interface LocalProjectCost {
  /** Project/repo (basename of the session cwd), or "unknown". */
  project: string;
  /** Dollar cost for the project, or null when its models have no canonical price. */
  cost: number | null;
  tokens: number;
}

export interface LocalApiValuePeriod {
  /** Estimated API value in USD (priced models only) - never a bill. */
  apiValueUsd: number;
  /** Processed tokens (fresh input + output + cache read/write). */
  tokens: number;
  /** Model tokens (input + output) with a canonical price. */
  pricedTokens: number;
  /** All model tokens (priced + unpriced) - the pricing-coverage denominator. */
  totalTokens: number;
  /** Whether the provider had any source data this period. */
  hasData: boolean;
}

export interface LocalApiValueProvider {
  providerId: string;
  today: LocalApiValuePeriod;
  yesterday: LocalApiValuePeriod;
  thirtyDays: LocalApiValuePeriod;
  /** Calendar days [today-60, today-30) for 30d dollar period-over-period. */
  priorThirtyDays: LocalApiValuePeriod;
  /** Last seven local calendar days, oldest first, today last. */
  lastSevenDays?: LocalApiValueDay[];
}

/** One local calendar day of estimated API value, for the card's trend. */
export interface LocalApiValueDay {
  /** Local calendar date as `YYYY-MM-DD`. */
  date: string;
  apiValueUsd: number;
  tokens: number;
}

export interface CursorModelActivity {
  /** Model id from Cursor's tracking (e.g. "grok-4.5", "default" for Auto). */
  model: string;
  /** Tracked AI code blocks attributed to this model. */
  contributions: number;
  /** Distinct Cursor requests that produced them. */
  requests: number;
}

export interface LocalUsageWindowRequest {
  id: string;
  label: string;
  startsAt: string;
  endsAt: string;
}

export interface LocalUsageWindowSummary extends LocalUsageWindowRequest {
  tokens: number;
  tokenBreakdown: LocalTokenBreakdown;
  /** Estimated API-value USD for this reset window from priced models only. */
  cost?: number | null;
}

export interface LocalUsageComparisonPeriod {
  id: string;
  label: string;
  currentTokens: number;
  currentBreakdown: LocalTokenBreakdown;
  previousTokens: number;
  previousBreakdown: LocalTokenBreakdown;
}

export interface LocalTokenBreakdown {
  processedTokens: number;
  freshInputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
}

export interface ProviderChartData {
  providerId: string;
  costHistory: DailyCostPoint[];
  creditsHistory: DailyCostPoint[];
  usageBreakdown: DailyUsageBreakdown[];
  localUsage: ProviderLocalUsageSummary | null;
  quotaHistory: UsageHistoryPoint[];
}

export interface UsageHistoryWindow {
  id: string;
  label: string;
  usedPercent: number;
}

export interface UsageHistoryPoint {
  recordedAt: string;
  windows: UsageHistoryWindow[];
}

// ── Token account types ──────────────────────────────────────────────

export interface TokenAccountSupportBridge {
  providerId: string;
  displayName: string;
  title: string;
  subtitle: string;
  placeholder: string;
}

export interface TokenAccountBridge {
  id: string;
  label: string;
  addedAt: string;
  lastUsed: string | null;
  isActive: boolean;
}

export interface ProviderTokenAccountsBridge {
  providerId: string;
  support: TokenAccountSupportBridge;
  accounts: TokenAccountBridge[];
  activeIndex: number;
}

// ── Phase 4 — provider ordering / cookie source / region ─────────────

export interface ProviderSummary {
  id: string;
  displayName: string;
  enabled: boolean;
  order: number;
}

// ── Phase 4 — credential detection ───────────────────────────────────

export interface GeminiCliStatus {
  signedIn: boolean;
  credentialsPath: string | null;
}

export interface VertexAiStatus {
  hasCredentials: boolean;
  credentialsPath: string | null;
}

export interface JetbrainsIde {
  id: string;
  displayName: string;
  path: string;
  detected: boolean;
}

export interface KiroStatus {
  available: boolean;
  hint: string | null;
}

// ── Phase 4 — session / environment ──────────────────────────────────

export interface WorkAreaRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

// ── Phase 5 — i18n ────────────────────────────────────────────────────

/** Snapshot returned by `get_locale_strings`. */
export interface LocaleStrings {
  language: Language;
  entries: Record<string, string>;
}

/** Payload emitted for `locale-changed`: the persisted language label. */
export type LocaleChangedPayload = Language;

// ── Phase 6b — provider detail pane ──────────────────────────────────

/** Aggregated per-provider payload powering the Settings detail pane. */
export interface ProviderDetail {
  id: string;
  displayName: string;
  enabled: boolean;

  // Identity
  email: string | null;
  plan: string | null;
  authType: string | null;
  sourceLabel: string | null;
  organization: string | null;
  lastUpdated: string | null;

  // Usage windows — mirror RateWindowSnapshot.
  session: RateWindowSnapshot | null;
  weekly: RateWindowSnapshot | null;
  modelSpecific: RateWindowSnapshot | null;
  tertiary: RateWindowSnapshot | null;
  extraRateWindows: Array<{
    id: string;
    title: string;
    window: RateWindowSnapshot;
  }>;

  cost: CostSnapshotBridge | null;
  pace: PaceSnapshot | null;

  lastError: string | null;

  dashboardUrl: string | null;
  statusPageUrl: string | null;
  buyCreditsUrl: string | null;

  hasSnapshot: boolean;

  /** Phase 6c — currently-persisted cookie source value ("auto" | "manual" | "off" | …).
   *  `null` for providers that do not expose a cookie-source picker. */
  cookieSource: string | null;
  /** Phase 6c — currently-persisted region value. `null` for non-regional providers. */
  region: string | null;
}

// ── Phase 6c — cookie-source & region pickers ────────────────────────

export interface CookieSourceOption {
  value: string;
  label: string;
  description?: string;
}

export interface RegionOption {
  value: string;
  label: string;
}

// ── Phase 6d — credential detection ──────────────────────────────────

export interface GeminiCliStatus {
  signedIn: boolean;
  credentialsPath: string | null;
}

export interface VertexAiStatus {
  hasCredentials: boolean;
  credentialsPath: string | null;
}

export interface JetbrainsIde {
  id: string;
  displayName: string;
  path: string;
  detected: boolean;
}

export interface KiroStatus {
  available: boolean;
  hint: string | null;
}
