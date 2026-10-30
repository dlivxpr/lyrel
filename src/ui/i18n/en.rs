use super::L10nKey;

pub fn translate_en(key: L10nKey) -> &'static str {
    match key {
        L10nKey::TerminalImageEvicted => "Image evicted",
        L10nKey::NotificationInbox => "Notifications",
        L10nKey::NotificationClearHistory => "Clear notification history",
        L10nKey::NotificationScope => "Received app notifications on this computer · latest 500",
        L10nKey::NotificationEmpty => {
            "No notifications. New application notifications will appear here."
        }
        L10nKey::NotificationSourceUnavailable => "Source unavailable",
        L10nKey::NotificationUnread => "Unread",
        L10nKey::NotificationSuccess => "Success",
        L10nKey::NotificationInfo => "Information",
        L10nKey::NotificationWarning => "Warning",
        L10nKey::NotificationError => "Error",
        L10nKey::SettingsNavSkills => "Skills",
        L10nKey::SkillsPageDescription => {
            "Browse omp, Kimi Code, pi, OpenCode 2.x and Hermes skills on the selected machine and project. Discover GitHub sources or skills.sh; review shared installation, check updates, preserve local changes and remove exact associated links."
        }
        L10nKey::SkillsProject => "Project",
        L10nKey::SkillsProjectHint => {
            "Absolute path on the selected machine; blank means user scope only"
        }
        L10nKey::SkillsApplyProject => "Apply project",
        L10nKey::SkillsOwnerProjectRequired => {
            "Switch the selected project to {project} before maintaining this installation"
        }
        L10nKey::SkillsSearch => "Search skills…",
        L10nKey::SkillsSource => "Source",
        L10nKey::SkillsAllSources => "All sources",
        L10nKey::SkillsBrowserDescription => "Browse skills on the selected machine and project",
        L10nKey::SkillsUserShared => "User shared",
        L10nKey::SkillsProjectShared => "Project shared",
        L10nKey::SkillsPrivate => "Agent private",
        L10nKey::SkillsRefresh => "Refresh",
        L10nKey::SkillsLoading => "Scanning the selected machine…",
        L10nKey::SkillsEmpty => {
            "No matching skills. Check the selected machine, project, native profile and source filter, then refresh. No configuration is changed."
        }
        L10nKey::SkillsIncomplete => "Incomplete or stale results",
        L10nKey::SkillsUnknown => "Unknown",
        L10nKey::SkillsDiscovered => "Discovered on disk",
        L10nKey::SkillsDisabled => "Not enabled by native source selection",
        L10nKey::SkillsRuntimeNote => {
            "Discovery does not prove a running agent loaded the skill. lyrel never changes native configuration or sends enable/reload commands to a terminal."
        }
        L10nKey::SkillsOpenFile => "Open SKILL.md",
        L10nKey::SkillsCopyPath => "Copy path",
        L10nKey::SkillsBrowseFiles => "Browse files…",
        L10nKey::SkillsOrigin => "Reported source",
        L10nKey::SkillsUpdated => "Updated",
        L10nKey::SkillsCompanions => "{files} companion files · {bytes}{limited}",
        L10nKey::SkillsContents => "Contents",
        L10nKey::SkillsNoDescription => "No description",
        L10nKey::SkillsMixedState => "Mixed native states",
        L10nKey::SkillsInvocation => "Argument hint",
        L10nKey::SkillsNoSource => "No verifiable source recorded",
        L10nKey::SkillsBack => "Back",
        L10nKey::SkillsCount => "{shown} of {total} skills",
        L10nKey::SkillsSelectSkill => "Select a skill to view its details",
        L10nKey::SkillsLocation => "Location",
        L10nKey::SkillsReadonly => "Agent-private installation · readonly",
        L10nKey::SkillsNextPage => "Next page",
        L10nKey::SkillsMachine => "Machine",
        L10nKey::SkillsMaintenanceTitle => "Review skill changes",
        L10nKey::SkillsNoMachine => {
            "Selected machine is disconnected. Reconnect it and refresh. The target has not changed."
        }
        L10nKey::SkillsReadError => "Could not read selected scope: {error}",
        L10nKey::SkillsProfileNote => {
            "Uses the selected Host environment and configured agent launch profile. Project paths never follow workspace changes automatically."
        }
        L10nKey::SkillsFileLimit => " (incomplete; links or scan limits)",
        L10nKey::SkillsCopied => "Path copied",
        L10nKey::SkillsInstallTitle => "Install skills",
        L10nKey::SkillsGithubSource => "GitHub repository",
        L10nKey::SkillsGithubHint => "owner/repo or https://github.com/owner/repo/tree/ref/path",
        L10nKey::SkillsRef => "Branch, tag or commit (optional)",
        L10nKey::SkillsRefHint => "Repository default; overrides the URL ref when specified",
        L10nKey::SkillsSubpath => "Repository subpath (optional)",
        L10nKey::SkillsSubpathHint => "Only discover skills below this relative directory",
        L10nKey::SkillsDiscover => "Discover skills",
        L10nKey::SkillsRegistryQuery => "Search skills.sh",
        L10nKey::SkillsRegistrySearch => "Search catalogue",
        L10nKey::SkillsRegistryNote => {
            "Uses the public search route used by skills CLI. Only GitHub sources are shown. Results still require repository discovery, selection and installation review."
        }
        L10nKey::SkillsInstallScope => "Shared installation scope",
        L10nKey::SkillsGithubAuthorization => {
            "GitHub discovery and downloads run on this desktop using its existing GH_TOKEN / GITHUB_TOKEN or gh authorization. Credentials never go to the selected Host. Review fixes that machine, home/project and source snapshot; cancel to change the destination."
        }
        L10nKey::SkillsGithubUnauthorized => {
            "GitHub rejected this desktop’s authorization (401). Refresh the existing environment or gh authorization and retry discovery."
        }
        L10nKey::SkillsGithubForbidden => {
            "GitHub denied access (403). Check this desktop’s repository permissions and organization SSO authorization."
        }
        L10nKey::SkillsGithubInvisible => {
            "Repository or ref is missing or not visible to this desktop’s authorization (404). Check the source and existing gh account permissions."
        }
        L10nKey::SkillsGithubRateLimited => {
            "GitHub rate limit reached. Wait for the limit to reset, then retry."
        }
        L10nKey::SkillsGithubNetwork => {
            "Could not reach GitHub. Check this desktop’s network and proxy, then retry."
        }
        L10nKey::SkillsInstallReview => "Review…",
        L10nKey::SkillsInstallConfirm => "Install",
        L10nKey::SkillsInstallReplace => "Replace this existing directory",
        L10nKey::SkillsInstallKeep => "Keep existing content (default)",
        L10nKey::SkillsInstallCancel => "Back to editing",
        L10nKey::SkillsInstallWorking => "Fetching or writing selected skills…",
        L10nKey::SkillsInstallEmpty => {
            "No candidates found. Try another repository, ref, subpath or search query."
        }
        L10nKey::SkillsInstallSafety => {
            "Writes only shared skills and the skills CLI source lock. No private links, native configuration changes or terminal commands. Existing names are kept unless individually approved for replacement."
        }
        L10nKey::SkillsInstallDone => "Installed: {name} — {path}",
        L10nKey::SkillsInstallRetained => "Kept unchanged: {name} — {path}",
        L10nKey::SkillsInstallDestination => "Review the fixed destination and source lock",
        L10nKey::SkillsInstallSelect => "Select skills to install",
        L10nKey::SkillsInstallCandidates => "Candidates: {count}",
        L10nKey::SkillsInstallBrowse => "Close",
        L10nKey::SkillsInstallError => "Could not install {name} — {path}: {error}",
        L10nKey::SkillsInstallRefResolved => "Resolved source: {source}",
        L10nKey::SkillsCheckUpdate => "Check for updates",
        L10nKey::SkillsUpdateAvailable => {
            "Update available — review this installation before replacing it"
        }
        L10nKey::SkillsUpdateCurrent => "Up to date — local content is kept",
        L10nKey::SkillsUpdateDeleted => {
            "The recorded skill is gone upstream. The local installation is kept; it will not be removed automatically."
        }
        L10nKey::SkillsUpdateUnknown => "Source cannot be verified — no update address is inferred",
        L10nKey::SkillsUpdateFailed => "Update check failed — this is not an up-to-date result",
        L10nKey::SkillsBaselineUnmodified => {
            "Local content matches the recorded baseline. Keep is still the default."
        }
        L10nKey::SkillsBaselineModified => {
            "Local changes found. Keep them unless you explicitly replace this entire installation; changes are not merged."
        }
        L10nKey::SkillsBaselineUnknown => {
            "The local baseline cannot be verified. Keep existing content by default; replacement does not merge your changes."
        }
        L10nKey::SkillsUpdateReplace => "Replace this installation and discard its current content",
        L10nKey::SkillsUpdateConfirm => "Replace",
        L10nKey::SkillsUpdateApplied => "Updated installation: {path}",
        L10nKey::SkillsRemoveReview => "Remove installation…",
        L10nKey::SkillsRemoveImpact => {
            "Remove this shared directory and its matching source record? Other tools reading this shared installation are affected too. Only the exact links listed below may be removed. Real copies, other targets and unverified links are kept. Execution rechecks the fixed Host, paths and content."
        }
        L10nKey::SkillsRemoveLinks => "Exact associated symbolic links / junctions",
        L10nKey::SkillsRemoveNoLinks => {
            "No proven associated links were found in the selected native roots."
        }
        L10nKey::SkillsRemoveConfirm => "Remove",
        L10nKey::SkillsRemoved => "Removed",
        L10nKey::SkillsMaintenanceWorking => {
            "Checking the fixed installation or applying the confirmed change…"
        }
        L10nKey::SkillsMaintenanceCancel => "Close",
        L10nKey::SkillsMaintenanceRetained => "Kept: {reason}",
        L10nKey::SkillsMaintenanceFailed => "Failed: {error}",
        L10nKey::SkillsMaintenanceUnknownOutcome => {
            "Outcome unknown: {error}. Inspect the listed paths before retrying; do not repeat the removal blindly."
        }
        L10nKey::StatisticsTitle => "Statistics",
        L10nKey::StatisticsKeywords => {
            "statistics omp pi kimi Kimi Code OpenCode Hermes native history tokens cost sessions machine profile date range overview source association add check remove price custom models.dev catalog currency cache"
        }
        L10nKey::StatisticsToday => "Today",
        L10nKey::StatisticsWeek => "This week",
        L10nKey::StatisticsMonth => "This month",
        L10nKey::StatisticsYear => "This year",
        L10nKey::StatisticsAll => "All time",
        L10nKey::StatisticsCustom => "Custom",
        L10nKey::StatisticsScope => {
            "Read native history on the selected machine, including sessions from other terminals. The machine does not follow the workspace."
        }
        L10nKey::StatisticsAgent => "Agent source",
        L10nKey::StatisticsAgentScope => {
            "All available sources or one agent’s effective scope. OpenCode uses the selected native client or configured service, without default-service fallback. Unreadable sources remain visible in diagnostics."
        }
        L10nKey::StatisticsReasoning => "Reasoning tokens",
        L10nKey::StatisticsRange => "Date range",
        L10nKey::StatisticsCalendar => {
            "Calendar boundaries use the desktop timezone; weeks start on Monday. Comparisons use the preceding matching calendar period."
        }
        L10nKey::StatisticsCustomDates => "Start / end date (YYYY-MM-DD, inclusive)",
        L10nKey::StatisticsApply => "Apply dates",
        L10nKey::StatisticsRefresh => "Refresh history",
        L10nKey::StatisticsLoading => "Scanning history…",
        L10nKey::StatisticsInvalidDate => {
            "Enter valid dates in YYYY-MM-DD format, with the start no later than the end."
        }
        L10nKey::StatisticsOffline => {
            "Machine offline — reconnect and refresh. Cached results are not current."
        }
        L10nKey::StatisticsReadError => {
            "Could not refresh native history. Previous results are stale. If the remote lacks agent environment metadata, upgrade lyrel-server. {error}"
        }
        L10nKey::StatisticsUpdated => {
            "Snapshot: {time} · desktop timezone: {zone}\nResult range: {range}"
        }
        L10nKey::StatisticsSnapshotTime => "Updated {time}",
        L10nKey::StatisticsStaleSnapshot => "Updated {time} · partial data",
        L10nKey::StatisticsComplete => "Complete",
        L10nKey::StatisticsPartial => "Partial data · see Data sources",
        L10nKey::StatisticsTokens => "Tokens",
        L10nKey::StatisticsCost => "Recorded historical cost",
        L10nKey::StatisticsEstimatedCost => "Native estimated cost (separate from actual)",
        L10nKey::StatisticsHermesLimits => {
            "Hermes uses the selected launcher's home/profile. Public history excludes child usage and has no pagination. Cumulative usage is included only when the range covers the entire session; it is never spread across days. Actual and native estimated costs are separate, not additive."
        }
        L10nKey::StatisticsHermesCoverage => {
            "Hermes public history excludes child usage, archived and zero-message sessions. Totals cover visible native CLI history only."
        }
        L10nKey::StatisticsHermesEmpty => {
            "Hermes returned an empty result. The public interface cannot distinguish empty history from an unavailable profile database."
        }
        L10nKey::StatisticsHermesWindow => {
            "Hermes reached a global or project preview window. The public interface has no offset/cursor; this is not complete history."
        }
        L10nKey::StatisticsHermesProfile => {
            "Hermes returned a different profile. The row was omitted; no fallback profile was used."
        }
        L10nKey::StatisticsHermesLineage => {
            "Hermes returned overlapping native lineage. Duplicate cumulative usage was omitted."
        }
        L10nKey::StatisticsHermesTime => {
            "Hermes stored usage has no reliable activity interval. Cumulative usage cannot be assigned to this date range."
        }
        L10nKey::StatisticsSessions => "Main sessions",
        L10nKey::StatisticsInput => "Input",
        L10nKey::StatisticsOutput => "Output",
        L10nKey::StatisticsCacheRead => "Cache read",
        L10nKey::StatisticsCacheWrite => "Cache write",
        L10nKey::StatisticsAverage => "Tokens per main session (including child usage)",
        L10nKey::StatisticsUnknown => "Unknown",
        L10nKey::StatisticsLastSevenDays => "Last 7 days",
        L10nKey::StatisticsDailyTrendCompact => "Daily · {start} – {end}",
        L10nKey::StatisticsActivityCaption => {
            "{metric} per day · dates outside the range are dimmed"
        }
        L10nKey::StatisticsActivityValues => {
            "Total tokens: {tokens}\nSpent {cost}\nSessions: {sessions}"
        }
        L10nKey::StatisticsLess => "Less",
        L10nKey::StatisticsMore => "More",
        L10nKey::StatisticsRankUsage => "{tokens} tokens · {sessions} sessions",
        L10nKey::StatisticsRankSpend => "{cost} · {tokens} tokens",
        L10nKey::StatisticsObservedValues => "Known subtotal",
        L10nKey::StatisticsObservedTokens => "Known token composition: {value}",
        L10nKey::UsageSubtitle => "Coding plan quota and API account balances",
        L10nKey::UsageCodingPlansCaption => "Periodic quota and Credits",
        L10nKey::UsageApiAccountsCaption => "Provider balances and account quota",
        L10nKey::UsageAccountsUnavailable => "Accounts could not be loaded",
        L10nKey::UsageAddHint => "Choose a source. Usage is verified before the account is saved.",
        L10nKey::UsageCurrentSource => "Current account and source",
        L10nKey::UsageReplaceHint => {
            "Choose and verify a new source to replace this account’s source."
        }
        L10nKey::UsageVerifyReplace => "Verify and replace",
        L10nKey::UsageVerifyImport => "Verify and import",
        L10nKey::StatisticsUnpriced => "Unpriced",
        L10nKey::StatisticsUnpricedCalls => {
            "Records without actual cost: {count}. Known actual costs are a subtotal, not a bill or account quota. Native estimates are separate. No prices are fetched or inferred."
        }
        L10nKey::StatisticsNotComparable => "Not comparable",
        L10nKey::StatisticsComparison => {
            "Previous period: {tokens} tokens · {cost} · {sessions} main sessions\nToken change: {change}"
        }
        L10nKey::StatisticsEmpty => {
            "No attributable usage in this range. Check the selected machine, source and date range, then refresh. Cumulative records crossing the range remain unassigned."
        }
        L10nKey::StatisticsPrivacy => {
            "Read-only native history · structured usage cache only; no prompts, conversation bodies or credentials. Source associations and prices are saved locally. Public prices are downloaded only after you enable models.dev and choose Check. Estimates are not bills, balances or account reconciliation. Coverage, lineage and pagination limits remain visible."
        }
        L10nKey::StatisticsSourcesTitle => "Sources",
        L10nKey::StatisticsSourcesHelp => {
            "Sources belong to the selected machine and agent scope. Removing an association never deletes native history. Check validates readability and coverage; failures keep previous data marked incomplete."
        }
        L10nKey::StatisticsAddSource => "Add source…",
        L10nKey::StatisticsCheckSource => "Check source",
        L10nKey::StatisticsRemoveSource => "Remove association",
        L10nKey::StatisticsSourceChecked => "Source checked; native history is readable.",
        L10nKey::StatisticsSourceSelector => {
            "omp / pi: absolute history directory or JSONL file. Kimi: absolute sessions directory. OpenCode: native service HTTP(S) URL without credentials or query. Hermes: resolved profile home directory, not a database."
        }
        L10nKey::StatisticsPricesTitle => "Token prices",
        L10nKey::StatisticsPricesHelp => {
            "Prices are per million tokens. Match provider, model and currency exactly. Leave unknown categories blank, not zero. Recorded costs, including zero, always win; custom prices precede enabled catalog prices. Partial coverage remains unpriced, and currencies are never combined."
        }
        L10nKey::StatisticsEditPrice => "Edit price…",
        L10nKey::StatisticsDeletePrice => "Delete price",
        L10nKey::StatisticsSavePrice => "Save price",
        L10nKey::StatisticsPriceProvider => "Exact provider ID",
        L10nKey::StatisticsPriceModel => "Exact model ID",
        L10nKey::StatisticsPriceCurrency => "Currency code (for example USD)",
        L10nKey::StatisticsPublicCatalog => "models.dev public prices",
        L10nKey::StatisticsPublicConsent => {
            "Disabled by default. Enabling permits manual checks of https://models.dev/api.json. Only the public catalog is requested; no history, paths, prompts or credentials are sent. No automatic checks run."
        }
        L10nKey::StatisticsCatalogEnable => "Enable public prices",
        L10nKey::StatisticsCatalogDisable => "Disable public prices",
        L10nKey::StatisticsCatalogCheck => "Check public prices",
        L10nKey::StatisticsCatalogStatus => {
            "Last check: {checked}\nCached at: {fetched} · {count} model prices. Failed checks keep this cache."
        }
        L10nKey::StatisticsPriceEstimate => {
            "{currency} · custom estimate {custom} · models.dev estimate {catalog}\n{priced} priced tokens · {unpriced} unpriced tokens · {partial} partially priced records"
        }
        L10nKey::StatisticsPricedCost => "Cost (recorded + explicit estimates)",
        L10nKey::StatisticsOpen => "Open statistics",
        L10nKey::StatisticsAnalysisTitle => "Activity and breakdowns",
        L10nKey::StatisticsAllAgents => "All agents",
        L10nKey::StatisticsAgents => "Agents",
        L10nKey::StatisticsModels => "Models",
        L10nKey::StatisticsProjects => "Projects",
        L10nKey::StatisticsAnalysisValues => {
            "{tokens} tokens · recorded {cost} · native estimate {estimated}\n{sessions} main sessions · {calls} usage records ({cumulative} cumulative) · {unpriced} unpriced records\nInput {input} · output {output} · reasoning {reasoning} · cache read {read} · cache write {write}\n{status}"
        }
        L10nKey::StatisticsAnalysisAgent => "Analysis agent",
        L10nKey::StatisticsAnalysisScope => {
            "Applies to this analysis only, within the machine, discovered sources and date range above. Older results remain visible while refreshing."
        }
        L10nKey::StatisticsAnalysisMetric => "Heatmap metric",
        L10nKey::StatisticsAnalysisError => "Could not build the desktop calendar: {error}",
        L10nKey::StatisticsPreviousYear => "Previous year",
        L10nKey::StatisticsNextYear => "Next year",
        L10nKey::StatisticsCalendarKeys => {
            "Select a date. Arrow keys: day/week · Page Up/Down: month · Home/End: year bounds"
        }
        L10nKey::StatisticsWeekdays => "Mon  Tue  Wed  Thu  Fri  Sat  Sun",
        L10nKey::StatisticsBreakdowns => "Breakdown",
        L10nKey::StatisticsBreakdownScope => {
            "Full selected range, not just the calendar year. Known token totals sort descending; unknowns have no rank. Agent, source scope and provider stay separate. Model/project sessions show participation, not additive totals. Child usage adds no main session."
        }
        L10nKey::StatisticsUnattributed => "Unattributed",
        L10nKey::StatisticsPreviousPage => "Previous page",
        L10nKey::StatisticsNextPage => "Next page",
        L10nKey::StatisticsAnalysisSnapshot => "Displayed snapshot analysis agent: {agent}",
        L10nKey::StatisticsNoAnalysisSources => {
            "No discovered source matches this analysis agent on the selected machine. Choose another agent or refresh; missing sources are not zero activity."
        }
        L10nKey::StatisticsStartDate => "Start date (YYYY-MM-DD)",
        L10nKey::StatisticsEndDate => "End date (YYYY-MM-DD)",
        L10nKey::StatisticsCostCard => "Recorded cost",
        L10nKey::StatisticsSessionColumn => "Sessions",
        L10nKey::StatisticsTokenDelta => "Previous period: {change}",
        L10nKey::StatisticsRecordedSubtotal => "{count} unpriced",
        L10nKey::StatisticsSessionAverage => "{value} tokens / main session",
        L10nKey::StatisticsRecordCount => "{calls} records · {cumulative} cumulative",
        L10nKey::StatisticsActivity => "Activity",
        L10nKey::StatisticsSourceSummary => "{count} locations · {issues} diagnostics",
        L10nKey::StatisticsPriceSummary => "{count} custom prices · public catalog {catalog}",
        L10nKey::StatisticsCatalogEnabled => "enabled",
        L10nKey::StatisticsCatalogDisabled => "off",
        L10nKey::StatisticsManageSources => "Sources…",
        L10nKey::StatisticsManagePrices => "Token prices…",
        L10nKey::StatisticsSaveSource => "Save source",
        L10nKey::StatisticsPriceRow => {
            "Input {input} · output {output} · reasoning {reasoning} · cache read {read} · cache write {write}"
        }
        L10nKey::StatisticsSourceLocation => "History path or native service URL",
        L10nKey::StatisticsAddPrice => "Add price…",
        L10nKey::StatisticsSourceTarget => "Target machine: {machine}",
        L10nKey::StatisticsCommon => "Frequent",
        L10nKey::StatisticsShare => "Share",
        L10nKey::StatisticsCompositionUnavailable => "Composition unavailable",
        L10nKey::StatisticsDailyTrend => "Daily · {start} – {end}; unknown days leave gaps",
        L10nKey::StatisticsTrendUnavailable => "Daily trend unavailable",
        L10nKey::StatisticsSummaryDetails => "Values and coverage",
        L10nKey::StatisticsRankContext => "{cost} · {sessions} sessions",
        L10nKey::IntegrationPiExtension => {
            "Pi 1.3.0 requires its native extension. --no-extensions/-ne disables discovery; --no-session disables history, resume and fork. Print/JSON/RPC modes are not the native TUI. Missing events never mean Done. Check startup diagnostics and project trust without bypassing consent."
        }
        L10nKey::IntegrationOmpExtension => {
            "OMP 18.12.1 requires the native extension in this profile. --no-extensions, disabledExtensions or --trusted-extension may exclude it. lyrel does not change authorization or extension policy. Installed files do not prove the running pane loaded the bridge."
        }
        L10nKey::IntegrationKimiHistory => {
            "Kimi Code 2.2.0 hooks track requests; final results require readable main-agent durable history. Stop hooks may be blocked and do not prove completion. Reload Kimi after changing hooks. Disabled hooks or persistence cannot report authoritative state."
        }
        L10nKey::IntegrationHermesObserver => {
            "Hermes observer home: {home}. Installation does not grant authorization. In this home/profile, run hermes plugins enable lyrel, then hermes plugins list. HERMES_SAFE_MODE or disabled plugins prevent reports. on_session_end carries finalized outcomes; on_session_finalize is not a turn result. Missing events never imply Idle/Done. Native history and operations use the selected installation's runtime, not the system Python. lyrel does not change plugin consent."
        }
        L10nKey::IntegrationScopeUnavailable => {
            "Integration scope is unavailable: {error}. No files were changed."
        }
        L10nKey::IntegrationOpenCodeUnreadable => {
            "Cannot read OpenCode CLI plugin configuration: {path}"
        }
        L10nKey::IntegrationOpenCodeRuntimeDiagnosis => {
            "OpenCode CLI configuration needs runtime diagnosis (JSONC or invalid JSON): {path}. Run opencode plugin list."
        }
        L10nKey::IntegrationOpenCodeDisabled => {
            "OpenCode CLI plugin lyrel is disabled by {pattern} in {path}. Authorization is unchanged."
        }
        L10nKey::AgentOutcomeSucceeded => "Succeeded",
        L10nKey::AgentOutcomeFailed => "Failed",
        L10nKey::AgentOutcomeInterrupted => "Interrupted",
        L10nKey::AgentOutcomeRejected => "Rejected",
        L10nKey::AgentResultWithReason => "{outcome}: {reason}",
        L10nKey::AgentResultWithDuration => "{result} — {secs}s",
        L10nKey::PendingSessionPreserved => {
            "Session {id} was created, but its pane could not open. The session is preserved. {error}"
        }
        L10nKey::PendingOpenSession => "Open preserved session",
        L10nKey::SettingsNoMatchesShort => "No matches",
        L10nKey::SettingsModifiedTitle => "Modified",
        L10nKey::SettingsMatchCount => "{count} matches",
        L10nKey::SettingsNothingModified => "Nothing changed from defaults",
        L10nKey::SettingsKeyThen => "then",
        L10nKey::SettingsStartupRestore => "Startup & restore",
        L10nKey::SettingsSettingsFile => "Settings file",
        L10nKey::SettingsReveal => "Reveal",
        L10nKey::SettingsThemeModeSystem => "System",
        L10nKey::SettingsThemeModeSystemDesc => {
            "Match the system, switching between a light and a dark theme."
        }
        L10nKey::SettingsThemeModeLightDesc => "Always use a light theme.",
        L10nKey::SettingsThemeModeDarkDesc => "Always use a dark theme.",
        L10nKey::SettingsThemeSlotLightDesc => "Used while the system is in light mode.",
        L10nKey::SettingsThemeSlotDarkDesc => "Used while the system is in dark mode.",
        L10nKey::SettingsThemeSlotDesc => "Theme for the terminal and interface.",
        L10nKey::SettingsLightThemeLabel => "Light theme",
        L10nKey::SettingsDarkThemeLabel => "Dark theme",
        L10nKey::SettingsNoThemesMatch => "No themes match",
        L10nKey::SettingsRestoreChanged => "Restore {count} changed",
        L10nKey::SettingsShortcutsHint => {
            "Click a shortcut and press new keys. Press a second chord right after to make a sequence. Esc cancels, ⌫ clears."
        }
        L10nKey::SettingsShortcutsHintTmux => {
            "tmux preset: pane and tab actions use a prefix key. Click a shortcut to change it; Esc cancels, ⌫ clears."
        }
        L10nKey::SettingsShortcutConflict => {
            "{keys} is already used by “{action}”. Replacing takes it from there."
        }
        L10nKey::SettingsReplace => "Replace",
        L10nKey::SettingsNoActionsMatch => "No actions match “{query}”",
        L10nKey::SettingsSearchShortcuts => "Search actions or keys",
        L10nKey::SettingsHostsDesc => "Recently used hosts. Search to find any saved host.",
        L10nKey::SettingsAddHost => "Add host",
        L10nKey::SettingsNoHostsMatch => "No hosts match “{query}”",
        L10nKey::SettingsHostsFromFiles => "{count} hosts from {files} sources",
        L10nKey::SettingsMoreHosts => "{count} more hosts",
        L10nKey::SettingsShowRecentOnly => "Show recent only",
        L10nKey::SettingsShowAll => "Show all",
        L10nKey::SettingsUnsaved => "Unsaved",
        L10nKey::SettingsNever => "Never",
        L10nKey::SettingsDefinedIn => "Defined in",
        L10nKey::SettingsConnectInNewTab => "Connect in new tab",
        L10nKey::SettingsCopied => "Copied",
        L10nKey::SettingsCopySshCommand => "Copy ssh command",
        L10nKey::SettingsStoredInLyrel => "Stored in lyrel settings",
        L10nKey::SettingsClickAgainToRemove => "Click again to remove",
        L10nKey::SettingsRemoveHost => "Remove host",
        L10nKey::SettingsEditHost => "Edit",
        L10nKey::SettingsLive => "Connected",
        L10nKey::SettingsPressKeysShort => "Press keys…",
        L10nKey::SettingsSearchAgents => "Search agents",
        L10nKey::SettingsConnectedMachines => "Connected",
        L10nKey::SettingsInstalledCount => "{count} installed",
        L10nKey::SettingsMachine => "Machine",
        L10nKey::SettingsMachineLocalDesc => {
            "Hooks are installed per machine. Remote machines appear here while connected."
        }
        L10nKey::SettingsMachineRemoteDesc => "Installing over the active connection to {name}.",
        L10nKey::SettingsAgentsInstalledSummary => "{count} of {total} installed",
        L10nKey::SettingsNoAgentsInstalled => "No agent hooks installed on this machine yet.",
        L10nKey::SettingsNoAgentsMatch => "No agents match “{query}”",
        L10nKey::SettingsMoreAgents => "{count} more agents available",
        L10nKey::SettingsShowInstalledOnly => "Show installed only",
        L10nKey::SettingsWorking => "Working…",
        L10nKey::SettingsUpdateAvailable => "Update available",
        L10nKey::SettingsRevealHookFile => "Reveal hook file",
        L10nKey::SettingsSaveError => "Changes could not be saved: {error}",
        L10nKey::SettingsRetrySave => "Retry saving",

        L10nKey::SettingsNavGeneral => "General",
        L10nKey::SettingsEditShortcuts => "Edit shortcuts…",
        L10nKey::SettingsModifiedOnly => "Modified only",
        L10nKey::SettingsModified => "Modified",
        L10nKey::SettingsResetValue => "Reset to default",
        L10nKey::SettingsSearchResults => "Search results",
        L10nKey::SettingsOpenSetting => "Open setting",
        L10nKey::SettingsNoModified => "No modified settings match this filter.",
        L10nKey::SettingsTerminalFontGroup => "Terminal text",
        L10nKey::SettingsInterfaceFontGroup => "Interface text",
        L10nKey::SettingsUnsavedTitle => "Save changes before leaving?",
        L10nKey::SettingsUnsavedBody => "Save your changes, discard them, or continue editing.",
        L10nKey::SettingsSaveChanges => "Save changes",
        L10nKey::SettingsThemeDraft => "Theme changes are previewed until you save.",

        L10nKey::SearchFiles => "Search files…",
        L10nKey::PanelSearchPlaceholder => "Search in files…",
        L10nKey::PanelSearchWholeWord => "Match whole word",
        L10nKey::PanelSearchIdle => "Search the contents of every file under:",
        L10nKey::PanelSearchNoFolder => "No folder to search.",
        L10nKey::PanelSearchNoFolderHint => {
            "Search looks through the project the active tab is in."
        }
        L10nKey::PanelSearchSshPane => "Search can't look inside an SSH pane's files.",
        L10nKey::PanelSearchSshPaneHint => {
            "Open the host as a remote workspace to search it, or browse it in Files."
        }
        L10nKey::PanelSearchSearching => "Searching…",
        L10nKey::PanelSearchNoMatches => "No results for “{query}”.",
        L10nKey::PanelSearchBadPattern => "Not a valid regular expression: {e}",
        L10nKey::PanelSearchServerTooOld => {
            "The lyrel-server on this machine is too old to search file contents."
        }
        L10nKey::PanelSearchServerTooOldHint => "Update the server on that host to use Search.",
        L10nKey::PanelSearchFailed => "Search failed: {e}",
        L10nKey::PanelSearchResultCount => "{count} results",
        L10nKey::PanelSearchFileCount => "{count} files",
        L10nKey::PanelSearchSummary => "{results} in {files}",
        L10nKey::PanelSearchTruncated => {
            "Not every match is shown. Narrow the search to see the rest."
        }
        L10nKey::PanelSearchLineTooltip => "Line {line}, column {column}",
        L10nKey::PanelSearchHostGone => "The machine this project is on is not connected.",
        L10nKey::SearchThemes => "Search themes…",
        L10nKey::SearchSettings => "Search settings…",
        L10nKey::FilterHosts => "Filter hosts…",
        L10nKey::SearchTheme => "Search…",
        L10nKey::SearchTabAll => "All",
        L10nKey::SearchTabActions => "Commands",
        L10nKey::SearchTabTerminals => "Terminals",
        L10nKey::SearchTabHosts => "Hosts",
        L10nKey::SearchTabSymbols => "Symbols",
        L10nKey::SearchPlaceholderAll => "Search files, actions, terminals and hosts…",
        L10nKey::SearchPlaceholderActions => "Search commands…",
        L10nKey::SearchPlaceholderTerminals => "Search open tabs, shells and agents…",
        L10nKey::SearchPlaceholderHosts => "Search hosts, or type user@host to connect…",
        L10nKey::SearchPlaceholderSymbols => "Go to a symbol in this file…",
        L10nKey::SearchSymbolsNone => "No symbols in this file",
        L10nKey::SearchSymbolsNoneHint => {
            "Symbols are listed for Rust, Go, Python, JavaScript, TypeScript, C, C++, Java, Ruby, shell and Markdown."
        }
        L10nKey::SearchTabFiles => "Files",
        L10nKey::SearchPlaceholderFiles => "Go to a file by name — add :line to jump to it…",
        L10nKey::SearchFilesNoRoots => "No project to search",
        L10nKey::SearchFilesNoRootsHint => {
            "Files are found in the project your terminal is in. cd into one to search it."
        }
        L10nKey::SearchFilesIndexing => "Indexing files…",
        L10nKey::SearchFilesFailed => "The project's files could not be listed.",
        L10nKey::SearchFilesGoToLine => "line {line}",
        L10nKey::SearchFilesCapped => "Large project — only the first {count} files are searched",
        L10nKey::CmdGoToFile => "Go to File…",
        L10nKey::SearchTabSessions => "Sessions",
        L10nKey::SearchPlaceholderSessions => "Search past agent sessions…",
        L10nKey::SearchSessionsEmptyHint => {
            "Past coding-agent sessions on this computer are listed here."
        }
        L10nKey::SearchSectionSessionsHere => "In {dir}",
        L10nKey::SearchSectionSessionsRecent => "Recent",
        L10nKey::AppSessionNotResumable => "{name} cannot resume a session by id.",
        L10nKey::AppSessionDirectoryGone => "The session's directory no longer exists: {path}",
        L10nKey::SearchSessionActions => "Choose what to do with this session…",
        L10nKey::SessionActionsHint => "actions",
        L10nKey::SessionActionResume => "Resume",
        L10nKey::SessionActionResumeSubtitle => "continue it in a new tab where it ran",
        L10nKey::SessionActionHide => "Remove from List",
        L10nKey::SessionActionHideSubtitle => "the agent's own history is kept",
        L10nKey::SearchMoreIn => "{count} more in {tab}",
        L10nKey::SearchNoResults => "No results",
        L10nKey::SearchSectionNewTerminal => "New Terminal",
        L10nKey::SearchWorkspacesAndMachines => "Search workspaces, tabs, and machines…",
        L10nKey::SearchFonts => "Search fonts…",
        L10nKey::SearchFind => "Find…",
        L10nKey::EditorSearchReplacement => "Replace…",
        L10nKey::EditorSearchReplaceAll => "Replace All",
        L10nKey::SearchMatchCase => "Match case",
        L10nKey::SearchUseRegex => "Use regular expression",
        L10nKey::NewFolderName => "New folder name",
        L10nKey::NewFileName => "New file name",
        L10nKey::HomeNewTab => "New Tab",
        L10nKey::HomeReopenClosedTab => "Reopen Closed Tab",
        L10nKey::HomeSwitchWorkspace => "Switch Workspace…",
        L10nKey::HomeSearchEverywhere => "Search…",
        L10nKey::HomeSplitRight => "Split Right",
        L10nKey::HomeSplitDown => "Split Down",
        L10nKey::HomeSettings => "Settings…",
        L10nKey::TrayQuitStopServer => "Quit and Stop Server…",
        L10nKey::Reconnect => "Reconnect",
        L10nKey::None => "None.",
        L10nKey::TryAgain => "Try Again",
        L10nKey::Refreshing => "refreshing…",
        L10nKey::Binary => "binary",
        L10nKey::Delete => "Delete",
        L10nKey::ConnectSshHint => "Type user@host to connect over SSH instead.",
        L10nKey::EditHint => "edit",
        L10nKey::OpenFileFromTree => "Open a file from the file tree",
        L10nKey::TreeDirLoading => "Reading…",
        L10nKey::TreeDirEmpty => "Empty",
        L10nKey::TreeDirHiddenOnly => "Only hidden files",
        L10nKey::TreeDirUnreadable => "Could not be read",
        L10nKey::TreeSearchCapped => "First {n} matches",
        L10nKey::TreeSearchFailed => "Search failed",
        L10nKey::FileChangedOnDisk => "File changed on disk",
        L10nKey::Reload => "Reload",
        L10nKey::KeepMine => "Keep mine",
        L10nKey::Dismiss => "Dismiss",
        L10nKey::StoredPasswordRejected => "The stored password was rejected. Enter a new one.",
        L10nKey::StoredPassphraseRejected => {
            "The saved passphrase did not unlock this key. Enter the right one."
        }
        L10nKey::Trust => "Trust",
        L10nKey::Abort => "Abort",
        L10nKey::HostKeyOverrideMessage => {
            "Type \"yes\" to override and trust the new key, or Esc to abort."
        }
        L10nKey::Override => "Override",
        L10nKey::RememberKeychain => "Remember (keychain)",
        L10nKey::Cancel => "Cancel",
        L10nKey::Close => "Close",
        L10nKey::QuitStopServerTitle => "Quit and stop lyrel server?",
        L10nKey::QuitStopServerBody => {
            "This quits lyrel and stops lyrel server; anything running in your shells is terminated. Your tabs and layout reopen with fresh shells next launch. (Closing the window only retires lyrel to the tray — the shells keep running.)"
        }
        L10nKey::QuitAndStop => "Quit and Stop",
        L10nKey::CloseSshConnectionTitle => "Close this SSH connection?",
        L10nKey::CloseSshConnectionBody => "The connection is live. Closing will end it.",
        L10nKey::ClosePaneBusyTitle => "Close this pane?",
        L10nKey::CloseTabBusyTitle => "Close this tab?",
        L10nKey::CloseBusyCommandBody => "{what} is still running. Closing ends it.",
        L10nKey::CloseBusyAgentBody => "{agent} is still working. Closing ends its turn.",
        L10nKey::CloseIdleBody => "Its shell ends with it.",
        L10nKey::CloseTabsTitle => "Close {count} tabs?",
        L10nKey::CloseTabsBody => "The shells in them end too.",
        L10nKey::Keep => "Keep",
        L10nKey::SettingsNavAppearance => "Appearance",
        L10nKey::SettingsNavTerminal => "Terminal",
        L10nKey::SettingsNavInput => "Keyboard & Mouse",
        L10nKey::SettingsNavSsh => "SSH",
        L10nKey::SettingsNavAgents => "Integrations",
        L10nKey::SettingsNavKeybindings => "Keyboard shortcuts",
        L10nKey::SettingsNavAbout => "About",
        L10nKey::SettingsNavUsage => "Usage",
        L10nKey::UsageSearchKeywords => {
            "Kimi Code Command Code DeepSeek StepFun Xiaomi MiMo balance plan Credits quota reset API key accounts rename refresh credentials usage private platform session cookie browser profile LocalStorage userToken access_token enhancement Firefox Safari Keychain Full Disk Access DPAPI App-Bound Encryption"
        }
        L10nKey::UsageScope => {
            "Desktop-wide query accounts, independent of SSH machines. No agent keys are read."
        }
        L10nKey::UsageOfficial => {
            "Query balances through the official api.deepseek.com API. Identity and reset periods are not provided."
        }
        L10nKey::UsageSource => "Authentication source",
        L10nKey::UsageProvider => "Provider",
        L10nKey::UsageKimiDisclosure => {
            "Private interfaces may change. API keys only go to api.kimi.com; web sessions only go to www.kimi.com. Sources are never switched automatically."
        }
        L10nKey::UsageKimiHelp => {
            "Copy access_token (raw or access_token JSON) from https://www.kimi.com LocalStorage in the selected profile. Only access_token and optional msh_user_id are read, never refresh_token or cookies. Browser material stays in request memory; manual tokens are saved only after validation."
        }
        L10nKey::UsageCommandDisclosure => {
            "Private interfaces on api.commandcode.ai only. Session enhancement must match the same account; failure leaves primary API-key usage intact."
        }
        L10nKey::UsageEnhance => "Configure session enhancement…",
        L10nKey::UsageEnhancementFailed => {
            "Session enhancement did not apply; primary API-key usage remains available"
        }
        L10nKey::UsageIdentity => "Provider identity",
        L10nKey::UsagePlan => "Plan",
        L10nKey::UsageRemaining => "Remaining",
        L10nKey::UsageReset => "Reset / period end",
        L10nKey::UsageFiveHour => "5-hour window",
        L10nKey::UsageWeekly => "Weekly window",
        L10nKey::UsageMonthlyTotal => "Monthly total quota",
        L10nKey::UsageMonthlyCode => "Monthly code quota",
        L10nKey::UsageMonthly => "Monthly credits",
        L10nKey::UsageFree => "Free credits",
        L10nKey::UsageBooster => "Booster wallet",
        L10nKey::UsageCoding => "Coding quota (provider-defined period)",
        L10nKey::UsageRequests => "requests",
        L10nKey::UsageUsed => "Used",
        L10nKey::UsageLimit => "Limit",
        L10nKey::UsageIdentityMismatch => {
            "The session and API key belong to different users. Enhancement was not saved; primary authentication is unchanged."
        }
        L10nKey::UsageCodeFiveHour => "Code 5-hour window",
        L10nKey::UsageCodeWeekly => "Code weekly window",
        L10nKey::UsageBoosterMonthly => "Booster monthly spending limit",
        L10nKey::UsageExpires => "Expires",
        L10nKey::UsageRawBalance => "Raw balance (provider units, unconverted)",
        L10nKey::UsageOverdrawn => "Provider reports this account is overdrawn",
        L10nKey::UsageCommandSessionHelp => {
            "Explicitly copy one session cookie as name=value from an authorized Command Code login request, not an API key or the entire Cookie header. It is saved only after api.commandcode.ai verifies the same user as the key and returns enhancement data. Cookie names and subscription.userId are not assumed."
        }
        L10nKey::UsageCommandBrowserHelp => {
            "Select a browser and profile, then verify. Only the Command Code session cookie is read, and /auth/get-session must match the primary API-key identity before billing enhancement. Browser material is never saved. Windows ABE is refused; traditional DPAPI is supported. macOS Chromium may request Keychain access only now. Safari uses best-effort binarycookies and requires Full Disk Access; background refresh never prompts and Safari requires explicit re-import. Session rotation is accepted only after identity verification."
        }
        L10nKey::UsageCommandCookieName => "Exact session cookie name",
        L10nKey::UsageCommandCookieNameHelp => {
            "Enter only the exact session cookie name from your authorized Command Code login request, not its value. Public BetterAuth defaults do not prove this server's cookie configuration. Lyrel does not guess a name: only your named cookie, in domains valid for api.commandcode.ai, is read and then verified against the API-key user. The name is a non-secret source selector."
        }
        L10nKey::UsageEnhancementBrowserSource => "Enhancement · browser session",
        L10nKey::UsageEnhancementManualSource => "Enhancement · manual session in credential store",
        L10nKey::UsageBrowserFullDiskAccess => {
            "Safari cookies require Full Disk Access for Lyrel in macOS Privacy & Security. Lyrel does not change system permissions or read Safari during background refresh. If you choose to grant access yourself, explicitly import the selected Safari source again."
        }
        L10nKey::UsageApiSource => "API key",
        L10nKey::UsageSessionSource => "Private platform · manual session",
        L10nKey::UsageBrowserSource => "Private platform · browser",
        L10nKey::UsageApiKey => "API key",
        L10nKey::UsageSessionInput => "Platform userToken or Harness grant",
        L10nKey::UsageSessionHelp => {
            "Copy userToken from platform.deepseek.com LocalStorage (raw value or userToken JSON), or paste Harness grant JSON with version, token and issuer. Cookies and official API keys are rejected. Only the required token enters the system credential store after validation."
        }
        L10nKey::UsagePrivateDisclosure => {
            "Private interfaces on platform.deepseek.com may change. Web userToken is not yet verified against first-party code or a real account. Harness grants require the matching issuer. Neither is an official API key."
        }
        L10nKey::UsageBrowser => "Browser",
        L10nKey::UsageProfile => "Browser profile",
        L10nKey::UsageChooseBrowser => "Choose a supported browser",
        L10nKey::UsageChooseProfile => "Choose a profile to verify and import",
        L10nKey::UsageBrowserHelp => {
            "Reads only userToken for https://platform.deepseek.com in the selected profile. Supports Chromium LevelDB and Firefox LocalStorage; Safari and the new Chromium SQLite format are unsupported. Material stays in memory, never in the vault, configuration, logs or temporary files. Refresh never requests system permission or switches profiles. Re-import after a source or token change."
        }
        L10nKey::UsageNormalWallet => "Platform topped-up wallet",
        L10nKey::UsageBonusWallet => "Platform bonus wallet",
        L10nKey::UsageBrowserLocked => {
            "The selected browser source is locked. Close that browser yourself and retry; no other source will be read."
        }
        L10nKey::UsageBrowserDenied => {
            "Access to the selected source was denied. Background refresh never requests permission. Check access or enter the session manually."
        }
        L10nKey::UsageBrowserMissing => {
            "The selected profile or provider session is missing. Sign in with that profile and import again."
        }
        L10nKey::UsageBrowserUnsupported => {
            "This platform or browser storage format is unsupported. Enter the session manually."
        }
        L10nKey::UsageBrowserChanged => {
            "The selected source or session changed; the account cannot be confirmed. Import again explicitly. No source was switched."
        }
        L10nKey::UsageBrowserUnreadable => {
            "Could not read the selected browser storage. No other profile was used. Check file access and browser version."
        }
        L10nKey::UsageStepFunHelp => {
            "From platform.stepfun.com, copy the decoded Oasis-Token request header and LocalStorage web_id. Paste Oasis-Token=…; Oasis-Webid=…. Web sessions only."
        }
        L10nKey::UsageMiMoHelp => {
            "Paste your platform.xiaomimimo.com session as api-platform_serviceToken=…; userId=… (serviceToken is also accepted). Web sessions only; balances and plan Credits stay separate."
        }
        L10nKey::UsageCookieInput => "Required session fields",
        L10nKey::UsageCookieBrowserHelp => {
            "Reads only the required cookies in the selected profile; StepFun also needs the exact platform origin's LocalStorage web_id. Firefox is supported; macOS Chromium requests Keychain access only during explicit import. Refresh never prompts. Windows App-Bound Encryption cannot be bypassed; use a manual session instead. Browser material is never saved. A changed session requires explicit re-import."
        }
        L10nKey::UsageCookieSource => "Private platform · browser cookies",
        L10nKey::UsageBrowserProtected => {
            "App-Bound Encryption protects these browser cookies. Lyrel cannot import them and will not weaken or bypass protection. Enter the session manually or select a supported Firefox profile."
        }
        L10nKey::UsageUnknownValue => "Unknown",
        L10nKey::UsageCash => "Cash balance",
        L10nKey::UsageGift => "Gift balance",
        L10nKey::UsageFrozen => "Frozen balance",
        L10nKey::UsageOverdraft => "Overdraft facility",
        L10nKey::UsageSubscriptionCredit => "Subscription Credits",
        L10nKey::UsageCreditBucket => "Credit bucket",
        L10nKey::UsageAdd => "Add account…",
        L10nKey::UsageCodingPlans => "Coding Plans",
        L10nKey::UsageApiAccounts => "API accounts",
        L10nKey::UsageProviderEmpty => "No accounts",
        L10nKey::UsageOfficialApi => "Official API",
        L10nKey::UsagePrivateApi => "Private API",
        L10nKey::UsageMore => "More account actions",
        L10nKey::UsageSave => "Save",
        L10nKey::UsageRemoveCommit => "Remove from lyrel",
        L10nKey::UsageRefreshAll => "Refresh all",
        L10nKey::UsageRefresh => "Refresh",
        L10nKey::UsageCadence => "Refresh cadence",
        L10nKey::UsageManual => "Manual only",
        L10nKey::UsageMinute => "Every minute",
        L10nKey::UsageFiveMinutes => "Every 5 minutes",
        L10nKey::UsageRename => "Rename…",
        L10nKey::UsageReplace => "Replace source…",
        L10nKey::UsageRemove => "Remove from lyrel…",
        L10nKey::UsageRemoveWarning => {
            "Only this local query account, its snapshots and its own credential slots are removed. The provider account and its credentials are not revoked."
        }
        L10nKey::UsageName => "Account name",
        L10nKey::UsageCancel => "Cancel",
        L10nKey::UsageVerify => "Verify and save",
        L10nKey::UsageLoading => "Query in progress",
        L10nKey::UsageRemoving => "Local removal incomplete — retry removal",
        L10nKey::UsageDisabled => "Refresh paused",
        L10nKey::UsageEnabled => "Refresh enabled",
        L10nKey::UsageBalance => "Balance",
        L10nKey::UsageGranted => "Granted",
        L10nKey::UsageToppedUp => "Topped up",
        L10nKey::UsageUpdated => "Updated:",
        L10nKey::UsageAvailable => "Provider reports sufficient balance for API calls",
        L10nKey::UsageUnavailable => "Provider reports insufficient balance for API calls",
        L10nKey::UsageOldSnapshot => "Previous snapshot — not a current successful query",
        L10nKey::UsageUnknown => "Unknown",
        L10nKey::UsageDisable => "Pause refresh",
        L10nKey::UsageEnable => "Enable refresh",
        L10nKey::UsageOpen => "Open usage",
        L10nKey::UsageAuthError => {
            "Authentication failed. Replace the credentials or explicitly re-import the selected source."
        }
        L10nKey::UsageRateError => "DeepSeek rate limit reached. Refresh later.",
        L10nKey::UsageNetworkError => {
            "Network request failed or timed out. Check the connection and proxy."
        }
        L10nKey::UsageCredentialError => {
            "System credential store failed. Existing usable credentials are retained; retry the operation."
        }
        L10nKey::UsageSaveError => {
            "Could not read or save local account metadata. Check directory access; existing data is not overwritten on read failure."
        }
        L10nKey::UsageResponseError => {
            "DeepSeek returned missing or unrecognized balance data. Balance remains unknown."
        }
        L10nKey::UsageServiceError => "DeepSeek returned a service error. Retry later.",
        L10nKey::UsageStaleError => "This account changed or was removed. Reopen the operation.",
        L10nKey::SettingsHeader => "SETTINGS",
        L10nKey::SettingsWindowTitle => "Settings",
        L10nKey::Reset => "Reset",
        L10nKey::Save => "Save",
        L10nKey::Connect => "Connect",
        L10nKey::Download => "Download",
        L10nKey::Link => "Link",
        L10nKey::SettingsThemeIntroTitle => "Theme",
        L10nKey::SettingsThemeIntroDesc => "Each theme sets its own light or dark look.",
        L10nKey::SettingsTypography => "Typography",
        L10nKey::SettingsFontSize => "Terminal font size",
        L10nKey::SettingsFontSizeDesc => "Terminal text size in points.",
        L10nKey::SettingsUiFontSize => "Interface font size",
        L10nKey::SettingsUiFontSizeDesc => "Text size for tabs, panels and settings.",
        L10nKey::SettingsUiFontFamily => "Interface font",
        L10nKey::SettingsUiFontFamilyDesc => "For tabs, sidebars, dialogs and settings.",
        L10nKey::SettingsLineHeight => "Line height",
        L10nKey::SettingsLineHeightDesc => "Row spacing as a multiple of the font size.",
        L10nKey::SettingsFontFamily => "Terminal font",
        L10nKey::SettingsFontFamilyDesc => "Pick from fonts installed on your system.",
        L10nKey::SettingsBoldFont => "Bold font",
        L10nKey::SettingsBoldFontDesc => "Bold face; Default derives it from the main font.",
        L10nKey::SettingsItalicFont => "Italic font",
        L10nKey::SettingsItalicFontDesc => "Italic face; Default derives it from the main font.",
        L10nKey::SettingsFontLigatures => "Font ligatures",
        L10nKey::SettingsFontLigaturesDesc => "Programming ligatures in terminal text.",
        L10nKey::SettingsFontThicken => "Thicken strokes",
        L10nKey::SettingsFontThickenDesc => "Draw text slightly bolder. Applies after a restart.",
        L10nKey::SettingsCursor => "Cursor",
        L10nKey::SettingsCursorShape => "Cursor shape",
        L10nKey::SettingsCursorShapeDesc => "How the terminal cursor is drawn.",
        L10nKey::SettingsPromptCursorShape => "Prompt cursor shape",
        L10nKey::SettingsPromptCursorShapeDesc => "The cursor at the shell prompt.",
        L10nKey::SettingsCursorBlink => "Cursor blink",
        L10nKey::SettingsCursorBlinkDesc => "Pulse the cursor while the terminal is focused.",
        L10nKey::SettingsLanguage => "Language",
        L10nKey::SettingsLanguageDesc => {
            "Follow the system UI language, or choose English or Simplified Chinese."
        }
        L10nKey::SettingsLanguageEnglish => "English",
        L10nKey::SettingsLanguageChinese => "简体中文",
        L10nKey::SettingsLanguageSystem => "Follow system",
        L10nKey::SettingsSearchLanguageKeywords => {
            "language, locale, english, chinese, follow system"
        }
        L10nKey::SettingsTransparency => "Transparency",
        L10nKey::SettingsOpacity => "Opacity",
        L10nKey::SettingsOpacityDesc => "Below 100%, the desktop shows through.",
        L10nKey::SettingsBlur => "Blur",
        L10nKey::SettingsBlurDesc => "Blur whatever is behind a translucent window.",
        L10nKey::SettingsBlurAutoDesc => "Blur behind a translucent window. Auto material only.",
        L10nKey::SettingsBackdrop => "Background material",
        L10nKey::SettingsBackdropDesc => "Native backdrop behind a translucent window.",
        L10nKey::SettingsSearchBackdropKeywords => {
            "material backdrop mica acrylic blur frosted window background"
        }
        L10nKey::SettingsBackdropAuto => "Auto",
        L10nKey::SettingsBackdropBlur => "Blur",
        L10nKey::SettingsBackdropMica => "Mica",
        L10nKey::SettingsBackdropMicaAlt => "Mica Alt",
        L10nKey::SettingsBackdropAcrylic => "Acrylic",
        L10nKey::SettingsBackdropOff => "Off",
        L10nKey::FollowTheme => "Follow theme",
        L10nKey::SettingsDimInactivePanes => "Dim inactive panes",
        L10nKey::SettingsDimInactivePanesDesc => {
            "Fade unfocused panes so the active one stands out."
        }
        L10nKey::SettingsOpenThemesFolder => "Open themes folder",
        L10nKey::SettingsChangeThemeImage => "Change…",
        L10nKey::SettingsChooseThemeImage => "Choose…",
        L10nKey::SettingsRemoveThemeImage => "Remove",
        L10nKey::SettingsImageOpacity => "Image opacity",
        L10nKey::SettingsImageOpacityDesc => {
            "How strongly the image shows over the background color."
        }
        L10nKey::SettingsEditTheme => "Edit theme",
        L10nKey::SettingsEditThemeIntro => {
            "Editing a copy. Changes apply live and save to its file."
        }
        L10nKey::SettingsBackgroundImage => "Background image",
        L10nKey::SettingsBackgroundImageDesc => {
            "Composited over the background color, under the text."
        }
        L10nKey::SettingsAnsiColors => "ANSI colors",
        L10nKey::SettingsCustomThemes => "Custom themes",
        L10nKey::SettingsThemesRejected => "Not loaded from the themes folder",
        L10nKey::ThemeDuplicateFailed => "Could not duplicate the theme",
        L10nKey::ThemeSaveFailed => "Could not save the theme",
        L10nKey::OpenInFileManagerFailed => "Could not open {path}",
        L10nKey::ExplorerMenuOpenIn => "Open in lyrel",
        L10nKey::ExplorerMenuOpenHere => "Open lyrel here",
        L10nKey::SettingsCustomThemesIntro => {
            "Duplicate a theme to edit it, or drop a YAML or .itermcolors file into the themes folder."
        }
        L10nKey::SettingsDuplicateToEdit => "Duplicate to edit",
        L10nKey::SettingsHosts => "Hosts",
        L10nKey::SettingsDefaults => "Defaults",
        L10nKey::SettingsInheritedByEveryHost => "Inherited by every host",
        L10nKey::SettingsNoSavedHosts => "No saved hosts yet.",
        L10nKey::SettingsNothingMatches => "Nothing matches {query}.",
        L10nKey::SettingsInLyrel => "lyrel settings",
        L10nKey::SettingsImportFromSshConfig => "Import from ~/.ssh/config",
        L10nKey::SettingsExpandAllGroups => "Expand All Groups",
        L10nKey::SettingsNoHostsYet => "No hosts yet",
        L10nKey::SettingsNothingSelected => "Nothing selected",
        L10nKey::SettingsTypeAddressToConnect => {
            "Type an address to connect now — lyrel offers to save it afterwards."
        }
        L10nKey::SettingsMoreInSshConfig => "{count} more in ~/.ssh/config",
        L10nKey::SettingsAliasesLinked => "{count} aliases linked.",
        L10nKey::SettingsImportAliases => "Import aliases",
        L10nKey::SettingsImportAliasesDesc => "Re-read the file and add anything new.",
        L10nKey::SettingsImportNow => "Import now",
        L10nKey::SettingsImportUnreadable => "Could not read {path} — nothing was imported.",
        L10nKey::SettingsImportNoHosts => {
            "{path} names no hosts to import — only wildcard or Match rules."
        }
        L10nKey::SettingsImportSummary => {
            "{count} hosts added — {updated} updated, {unchanged} already current"
        }
        L10nKey::SettingsImportIgnored => {
            "{count} options have no setting in lyrel and were left in the file: {options}"
        }
        L10nKey::SettingsImportMoreOptions => "+{count} more",
        L10nKey::SettingsDefaultsIntro => {
            "Every host starts here; override any under its Advanced."
        }
        L10nKey::SettingsCopyAddress => "Copy Address",
        L10nKey::SettingsDuplicate => "Duplicate",
        L10nKey::SettingsForgetPassword => "Forget Password",
        L10nKey::SettingsForgetPasswordTitle => "Forget the saved password for {endpoint}?",
        L10nKey::SettingsForgetPasswordBody => {
            "The next connection to it asks for the password again. Nothing else about this host changes."
        }
        L10nKey::SettingsForgetPasswordSharedBody => {
            "{count} other host profiles use {endpoint} as well, so those connections will have to enter the password again too."
        }
        L10nKey::SettingsForgotPasswordFor => "Forgot saved password for {endpoint}",
        L10nKey::SettingsDeleteProfileBody => {
            "The password saved for it goes too, unless another connection still uses the same address."
        }
        L10nKey::SettingsDeleteProfileCascade => {
            "{count} saved remote workspace entries point at {endpoint} and go with it. The sessions on the remote machine keep running — connect with a new profile and they reappear in the workspace list."
        }
        L10nKey::SettingsCouldntForgetPassword => {
            "Could not forget the saved password for {endpoint}: {error}"
        }
        L10nKey::SettingsSecurity => "Security",
        L10nKey::SettingsSecurityIntro => "Any host can override these under its Advanced.",
        L10nKey::SettingsVerifyHostKeys => "Verify host keys",
        L10nKey::SettingsVerifyHostKeysDesc => {
            "Check server keys against known_hosts. Off, a spoofed server goes unnoticed."
        }
        L10nKey::WarnBeforeClosing => "Warn before closing",
        L10nKey::SettingsWarnBeforeClosingDesc => {
            "Confirm before closing a tab with a live SSH session."
        }
        L10nKey::SettingsNewHost => "New host",
        L10nKey::SettingsDiscardChangesTitle => "Discard unsaved changes?",
        L10nKey::SettingsDiscardChangesBody => {
            "The connection you are editing has changes that were never saved."
        }
        L10nKey::SettingsKeepEditing => "Keep Editing",
        L10nKey::SettingsName => "Name",
        L10nKey::SettingsHost => "Host",
        L10nKey::SettingsHostRequired => "Needs a host — won't be saved.",
        L10nKey::SettingsPortInvalid => "Port must be 1-65535 — blank means 22.",
        L10nKey::SettingsUser => "User",
        L10nKey::SettingsAuth => "Auth",
        L10nKey::SettingsAuthDesc => "Auto tries every applicable method.",
        L10nKey::SettingsAuthModeAuto => "Auto",
        L10nKey::SettingsAuthModePassword => "Password",
        L10nKey::SettingsAuthModeKey => "Key",
        L10nKey::SettingsAuthModeAgent => "Agent",
        L10nKey::SettingsAuthMode2Fa => "2FA",
        L10nKey::SettingsPassword => "Password",
        L10nKey::SettingsNameHint => "Optional label",
        L10nKey::SettingsHostHint => "hostname or IP",
        L10nKey::SettingsUserHint => "resolved at connect",
        L10nKey::SettingsPasswordDesc => "Kept in the system keychain, never in the config file.",
        L10nKey::SettingsPasswordHint => "Ask when connecting",
        L10nKey::SettingsKeyPassphrase => "Key passphrase",
        L10nKey::SettingsKeyPassphraseDesc => "Unlocks the key above. Kept in the system keychain.",
        L10nKey::SettingsPassphraseNeedsKey => {
            "Name a key file first — a passphrase is stored against the key it unlocks."
        }
        L10nKey::SettingsBrowseKey => "Browse…",
        L10nKey::SettingsCouldntSavePassword => {
            "Could not save the password for {endpoint}: {error}"
        }
        L10nKey::SettingsCouldntSavePassphrase => {
            "Could not save the passphrase for {key}: {error}"
        }
        L10nKey::SettingsJumpHost => "Jump host",
        L10nKey::SettingsJumpHostDesc => "Profile to tunnel through; blank connects directly.",
        L10nKey::SettingsJumpHostUnknown => "No host profile named {jump_name} — won't be saved.",
        L10nKey::SettingsJumpHostSelf => "A host can't be its own jump host — won't be saved.",
        L10nKey::SettingsNoneSummary => "(none)",
        L10nKey::SettingsPortForwarding => "Port forwarding",
        L10nKey::SettingsRulesOpenedWithConnection => "1 rule, opened with the connection",
        L10nKey::SettingsAddRule => "+ Add rule",
        L10nKey::SettingsRemoveRule => "Remove rule",
        L10nKey::SettingsFwdLegendLocal => "L — a local port reaches the remote side",
        L10nKey::SettingsFwdLegendRemote => "R — a remote port reaches this machine",
        L10nKey::SettingsFwdLegendDynamic => "D — dynamic SOCKS proxy",
        L10nKey::SettingsFwdNeedsBoth => {
            "Needs a listen port and a target host:port — won't be saved."
        }
        L10nKey::SettingsFwdNeedsListen => "Needs a listen port — won't be saved.",
        L10nKey::SettingsAdvanced => "Advanced",
        L10nKey::SettingsAdvancedSummary => {
            "algorithms / keepalive / proxies / X11 / login scripts"
        }
        L10nKey::SettingsGroupAuthentication => "Authentication",
        L10nKey::SettingsGroupProxies => "Proxies",
        L10nKey::SettingsGroupAlgorithms => "Algorithms",
        L10nKey::SettingsGroupConnection => "Connection",
        L10nKey::SettingsGroupSession => "Session",
        L10nKey::SettingsGroupSecurity => "Security",
        L10nKey::SettingsRemoteClipboardWrite => "Remote clipboard images",
        L10nKey::SettingsRemoteClipboardWriteDesc => {
            "Let this host put images on your clipboard (OSC 5522)."
        }
        L10nKey::SettingsIdentityFiles => "Identity files",
        L10nKey::SettingsIdentityFilesDesc => "Private-key paths, one per line (%h/%r expand).",
        L10nKey::SettingsAgentForwarding => "Agent forwarding",
        L10nKey::SettingsAgentForwardingDesc => "Forward the local ssh-agent to the connection.",
        L10nKey::SettingsProxyCommand => "ProxyCommand",
        L10nKey::SettingsProxyCommandDesc => "Transport command (%h/%p/%r substituted).",
        L10nKey::SettingsSocks5Proxy => "SOCKS5 proxy",
        L10nKey::SettingsSocks5ProxyDesc => "host:port (blank = none).",
        L10nKey::SettingsHttpProxy => "HTTP proxy",
        L10nKey::SettingsHttpProxyDesc => "host:port (blank = none).",
        L10nKey::SettingsProxyOverridden => "Not used: {winner} comes first.",
        L10nKey::SettingsTestConnection => "Test",
        L10nKey::SettingsTestRunning => "Testing the connection…",
        L10nKey::SettingsTestReached => "Connected and authenticated in {time}.",
        L10nKey::SettingsTestNeedsPassword => {
            "Reached the server — it asked for a password. Connect to type it."
        }
        L10nKey::SettingsTestNeedsPassphrase => {
            "Reached the server — the private key asked for its passphrase. Connect to type it."
        }
        L10nKey::SettingsTestNeedsInteractive => {
            "Reached the server — it asked for a keyboard-interactive answer. Connect to give it."
        }
        L10nKey::SettingsTestNeedsHostKey => {
            "Reached the server — its host key has not been accepted yet. Connect once to review it."
        }
        L10nKey::SettingsTestHostKeyChanged => {
            "Reached the server — its host key is not the one it gave before. Connect once to review the change."
        }
        L10nKey::SettingsTestFailed => "Did not connect: {reason}",
        L10nKey::SettingsProxyPortInvalid => {
            "Port must be 1-65535 — the host on its own takes the default port."
        }
        L10nKey::SettingsKexAlgorithms => "KEX algorithms",
        L10nKey::SettingsKexAlgorithmsDesc => "Comma-separated (blank = library default).",
        L10nKey::SettingsCiphers => "Ciphers",
        L10nKey::SettingsCiphersDesc => "Comma-separated (blank = default).",
        L10nKey::SettingsMacs => "MACs",
        L10nKey::SettingsMacsDesc => "Comma-separated (blank = default).",
        L10nKey::SettingsHostKeyAlgorithms => "Host-key algorithms",
        L10nKey::SettingsHostKeyAlgorithmsDesc => "Comma-separated (blank = default).",
        L10nKey::SettingsCompression => "Compression algorithms",
        L10nKey::SettingsJumpHostVia => "via {jump_name}",
        L10nKey::SettingsConnected => "connected",
        L10nKey::SettingsProfileCopied => "{name} (copy)",
        L10nKey::SettingsCompressionDesc => "Comma-separated (blank = default).",
        L10nKey::SettingsKeepaliveInterval => "Keepalive interval (s)",
        L10nKey::SettingsKeepaliveIntervalDesc => "Blank = library default.",
        L10nKey::SettingsKeepaliveCountMax => "Keepalive count max",
        L10nKey::SettingsKeepaliveCountMaxDesc => "Missed keepalives before dead.",
        L10nKey::SettingsConnectTimeout => "Connect timeout (s)",
        L10nKey::SettingsConnectTimeoutDesc => "Blank = library default.",
        L10nKey::SettingsX11Forwarding => "X11 forwarding",
        L10nKey::SettingsX11ForwardingDesc => {
            if cfg!(target_os = "macos") {
                "Request X11 forwarding (needs XQuartz)."
            } else if cfg!(target_os = "windows") {
                "Request X11 forwarding (needs an X server running, such as VcXsrv or X410)."
            } else {
                "Request X11 forwarding."
            }
        }
        L10nKey::SettingsShellIntegration => "Shell integration",
        L10nKey::SettingsShellIntegrationDesc => {
            "Remote shell reports prompts, exit codes and directory."
        }
        L10nKey::SettingsLoginScripts => "Login scripts",
        L10nKey::SettingsLoginScriptsDesc => "Commands sent after the shell opens, one per line.",
        L10nKey::SettingsSkipBanner => "Skip banner",
        L10nKey::SettingsSkipBannerDesc => "Suppress the server login banner.",
        L10nKey::SettingsDefaultFollowsDefaults => "Default follows Defaults, which is {value}.",
        L10nKey::SettingsValueOn => "on",
        L10nKey::SettingsValueOff => "off",
        L10nKey::SettingsDefault => "Default",
        L10nKey::SettingsOn => "On",
        L10nKey::SettingsOff => "Off",
        L10nKey::SettingsShell => "Shell",
        L10nKey::SettingsShellIntro => "The program new terminals launch. Empty uses {default}.",
        L10nKey::SettingsProgram => "Shell program",
        L10nKey::SettingsProgramDesc => "A name on PATH or an absolute path, e.g. zsh, fish.",
        L10nKey::SettingsArguments => "Shell arguments",
        L10nKey::SettingsArgumentsDesc => "Split like a command line, e.g. -l or -c \"echo hi\".",
        L10nKey::SettingsArgumentsInvalid => {
            "The quotes do not balance — this value was not saved."
        }
        L10nKey::SettingsStartIn => "Starting directory",
        L10nKey::SettingsStartInDesc => "Launch directory, home folder, or a fixed path.",
        L10nKey::SettingsCustomPath => "Custom path",
        L10nKey::SettingsCustomPathDesc => "The directory new shells start in.",
        L10nKey::SettingsWdInherit => "Inherit",
        L10nKey::SettingsWdHome => "Home",
        L10nKey::SettingsWdCustom => "Custom",
        L10nKey::SettingsWdPathInvalid => {
            "That directory does not exist — the value was not saved."
        }
        L10nKey::SettingsShellFooter => {
            "Only for shells with nothing to inherit, like a window's first tab. New tabs and splits take the active pane's directory."
        }
        L10nKey::SettingsScrolling => "Scrolling",
        L10nKey::SettingsScrollback => "Scrollback buffer",
        L10nKey::SettingsScrollbackDesc => "Lines of history kept per pane. Applies to new panes.",
        L10nKey::SettingsScrollSpeed => "Scroll speed",
        L10nKey::SettingsScrollSpeedDesc => "Multiplier applied to mouse-wheel scrolling.",
        L10nKey::SettingsSmoothScroll => "Smooth scrolling",
        L10nKey::SettingsSmoothScrollDesc => {
            "Ease each wheel notch into place. Trackpads are unaffected."
        }
        L10nKey::SettingsMouse => "Mouse",
        L10nKey::SettingsFocusFollowsMouse => "Focus follows mouse",
        L10nKey::SettingsFocusFollowsMouseDesc => "Hovering a pane focuses it without a click.",
        L10nKey::SettingsHideMouseWhileTyping => "Hide mouse while typing",
        L10nKey::SettingsHideMouseWhileTypingDesc => {
            "Hide the pointer while typing; moving brings it back."
        }
        L10nKey::SettingsMouseZoom => "Zoom with the wheel",
        L10nKey::SettingsMouseZoomDesc => "Hold it and scroll to resize the terminal font.",
        L10nKey::SettingsMouseZoomOff => "Off",
        L10nKey::SettingsReportMouseToApps => "Report mouse to apps",
        L10nKey::SettingsReportMouseToAppsDesc => {
            "Let vim, tmux and other apps handle clicks; Shift keeps it local."
        }
        L10nKey::SettingsBell => "Bell",
        L10nKey::SettingsTerminalBell => "Terminal bell",
        L10nKey::SettingsTerminalBellDesc => "How a bell (^G) is signalled.",
        L10nKey::SettingsLinks => "Links",
        L10nKey::DetectUrls => "Detect URLs",
        L10nKey::SettingsDetectUrlsDesc => "Underline links on hover; {modifier}-click opens them.",
        L10nKey::ForwardSshLoopbackLinks => "Forward remote ports",
        L10nKey::SettingsForwardSshLoopbackLinksDesc => {
            "Open a remote pane's localhost links here."
        }
        L10nKey::SettingsOpenFilesInternal => "Built-in editor",
        L10nKey::SettingsOpenFilesSystem => "Default app",
        L10nKey::SettingsOpenFilesCommand => "Command",
        L10nKey::SettingsOpenFilesModeDesc => {
            "What {modifier}-clicking a file link opens. Only the built-in editor jumps to a line or opens remote files."
        }
        L10nKey::LinkFileNotUnder => "{path} — nothing by that name under {dir}",
        L10nKey::LinkFileNoDirectory => {
            "{path} — this pane has not said which directory it is in, so a relative path has no base"
        }
        L10nKey::LinkFileMissing => "{path} — nothing at that path",
        L10nKey::LinkDirOutsideTree => {
            "{path} — on another machine, and outside every folder the Files panel has open"
        }
        L10nKey::OpenFilesWith => "Open files with",
        L10nKey::SettingsOpenFilesWithDesc => {
            "Use {path}, {line}, {column}. Empty uses the default app."
        }
        L10nKey::SettingsBellModeOff => "Off",
        L10nKey::SettingsBellModeVisual => "Visual",
        L10nKey::SettingsBellModeAudible => "Audible",
        L10nKey::SettingsBellModeBoth => "Both",
        L10nKey::SettingsPrompt => "Prompt & command history",
        L10nKey::SettingsPromptIntro => {
            "lyrel's own editor and menus at the prompt. Turn one off to use the shell's."
        }
        L10nKey::SettingsPromptEditor => "lyrel prompt editor",
        L10nKey::SettingsPromptEditorDesc => {
            "Selection, undo and menus on the line you type. Off hands it back to ZLE, readline or fish."
        }
        L10nKey::SettingsNeedsPromptEditor => {
            "Needs the prompt editor: with it off, this key already belongs to the shell."
        }
        L10nKey::SettingsTabCompletion => "Tab completion",
        L10nKey::SettingsTabCompletionDesc => {
            "Tab opens lyrel's completion menu. Off, Tab goes to the shell."
        }
        L10nKey::SettingsHistorySearch => "Command history search",
        L10nKey::SettingsHistorySearchDesc => {
            "⌃R opens lyrel's fuzzy history search. Off, ⌃R goes to the shell."
        }
        L10nKey::SettingsSelectionClipboard => "Selection & clipboard",
        L10nKey::SettingsSmartSelection => "Smart selection",
        L10nKey::SettingsSmartSelectionDesc => {
            "Double-click selects a whole URL, path or bracket pair."
        }
        L10nKey::SettingsCopyOnSelect => "Copy on select",
        L10nKey::SettingsCopyOnSelectDesc => {
            if cfg!(target_os = "macos") {
                "Selecting text with the mouse copies it to the clipboard right away, no ⌘C needed."
            } else {
                "Selecting text with the mouse copies it to the clipboard right away, no Ctrl+Shift+C needed."
            }
        }
        L10nKey::SettingsTrimTrailingSpaces => "Trim trailing spaces on copy",
        L10nKey::SettingsTrimTrailingSpacesDesc => {
            "Strip trailing whitespace from each copied line."
        }
        L10nKey::SettingsKeyboard => "Keyboard",
        L10nKey::SettingsOptionAsMeta => "Option (⌥) acts as Meta",
        L10nKey::SettingsOptionAsMetaDesc => "⌥+key sends Meta, e.g. ⌥B moves back a word.",
        L10nKey::SettingsAgentsIntro => "Agent hooks",
        L10nKey::SettingsAgentsIntroDesc => {
            "Hooks show each agent's status — working, waiting, done — in the tab bar."
        }
        L10nKey::SettingsReadingAgentConfig => "Reading this machine's agent config…",
        L10nKey::SettingsStatusNotInstalled => "Not installed",
        L10nKey::SettingsStatusInstalled => "Installed",
        L10nKey::SettingsStatusOutdated => "Outdated",
        L10nKey::SettingsInstall => "Install",
        L10nKey::SettingsReinstall => "Reinstall",
        L10nKey::SettingsUpdate => "Update",
        L10nKey::SettingsUninstall => "Uninstall",
        L10nKey::SettingsOfflineMachines => {
            "{count} more saved machines are not connected — open a workspace on one to install its hooks there."
        }
        L10nKey::SettingsSyncWithSystem => "Appearance",
        L10nKey::SettingsSyncWithSystemDesc => "Separate light and dark themes that follow the OS.",
        L10nKey::SettingsLegiblePalette => "Legible bright colors",
        L10nKey::SettingsLegiblePaletteDesc => "Fix bright ANSI colors that are hard to read.",
        L10nKey::SettingsChangeTheme => "Change theme",
        L10nKey::SettingsThemes => "Themes",
        L10nKey::SettingsThemesCloseTooltip => "Close Themes (Esc)",
        L10nKey::SettingsThemePanelManual => "Change your current theme.",
        L10nKey::SettingsThemePanelLight => "Choose the theme for light mode.",
        L10nKey::SettingsThemePanelDark => "Choose the theme for dark mode.",
        L10nKey::SettingsCustom => "Custom",
        L10nKey::SettingsCustomValue => "Custom ({value})",
        L10nKey::SettingsBuiltIn => "Built-in",
        L10nKey::SettingsDark => "Dark",
        L10nKey::SettingsLight => "Light",
        L10nKey::SettingsLightMode => "Light mode",
        L10nKey::SettingsDarkMode => "Dark mode",
        L10nKey::SettingsActive => "Active",
        L10nKey::SettingsStartupWindow => "Startup window",
        L10nKey::SettingsStartupWindowDesc => "Window state when lyrel launches.",
        L10nKey::SettingsRememberWindowSize => "Remember window size & position",
        L10nKey::SettingsRememberWindowSizeDesc => {
            "Reopen where the window was when lyrel last quit."
        }
        L10nKey::SettingsRestoreLastLayout => "Restore last layout",
        L10nKey::SettingsRestoreLastLayoutDesc => {
            "Reopen last session's tabs, splits and directories."
        }
        L10nKey::SettingsShowTrayIcon => "Show tray icon",
        L10nKey::SettingsShowTrayIconDesc => "Alerts when an agent needs input; jumps to its pane.",
        L10nKey::SettingsTabs => "Tabs",
        L10nKey::SettingsNewTabPosition => "New tab position",
        L10nKey::SettingsNewTabPositionDesc => "Where a freshly opened tab is inserted.",
        L10nKey::SettingsConfirmClose => "Confirm before closing",
        L10nKey::SettingsConfirmCloseDesc => {
            "When closing a tab or pane asks first. SSH hosts set to warn before closing are always asked about."
        }
        L10nKey::ConfirmCloseNever => "Never",
        L10nKey::ConfirmCloseWhenBusy => "When busy",
        L10nKey::ConfirmCloseAlways => "Always",
        L10nKey::SettingsSidebarGrouping => "Auto grouping",
        L10nKey::SettingsSidebarGroupingDesc => {
            "Group unpinned tabs by git repository, SSH tabs by host."
        }
        L10nKey::DocumentDock => "Dock beside terminal",
        L10nKey::DocumentFill => "Fill window",
        L10nKey::SettingsNotifications => "Notifications",
        L10nKey::SettingsNotifyOnCommandFinish => "Notify on command finish",
        L10nKey::SettingsNotifyOnCommandFinishDesc => {
            "Alert when a long foreground command finishes."
        }
        L10nKey::SettingsNotifyThreshold => "Minimum command duration",
        L10nKey::SettingsNotifyThresholdDesc => {
            "Notify only when a command runs for at least this long."
        }
        L10nKey::SettingsWindow => "Startup & restore",
        L10nKey::NotifyModeNever => "Never",
        L10nKey::NotifyModeUnfocused => "When unfocused",
        L10nKey::NotifyModeAlways => "Always",
        L10nKey::ProgramNotesDropped => "More notifications from this pane weren't shown",
        L10nKey::SettingsStartupNormal => "Normal",
        L10nKey::SettingsStartupMaximized => "Maximized",
        L10nKey::SettingsStartupFullscreen => "Fullscreen",
        L10nKey::SettingsAfterCurrent => "After current",
        L10nKey::SettingsAtEnd => "At end",
        L10nKey::SettingsPreset => "Preset",
        L10nKey::SettingsPresetDesc => "tmux maps pane and tab actions to prefix keys (Ctrl-B C).",
        L10nKey::SettingsPrefix => "Prefix",
        L10nKey::SettingsPressKeys => "Press keys… · ⌫ for no shortcut",
        L10nKey::SettingsPauseToSaveEsc => "pause to save · Esc",
        L10nKey::SettingsKeybindingsIntroDesc => {
            "Click a shortcut and press new keys; chain keys for sequences like Ctrl-B X. Esc cancels, Backspace clears."
        }
        L10nKey::SettingsPrefixNote => "A lone prefix key reaches the shell after ~1s.",
        L10nKey::SettingsRestoreAllDefaults => "Restore all defaults",
        L10nKey::SettingsRestoreAllDefaultsBody => {
            "Every key you have rebound goes back to its default. There is no undo."
        }
        L10nKey::KeybindGoToTab => "Go to Tab {n}",
        L10nKey::KeybindGoToWorkspace => "Go to Workspace {n}",
        L10nKey::KeybindInsertNewline => "Insert Newline",
        L10nKey::KeybindForkSessionRight => "Fork Session Right",
        L10nKey::KeybindForkSessionLeft => "Fork Session Left",
        L10nKey::KeybindForkSessionDown => "Fork Session Down",
        L10nKey::KeybindForkSessionUp => "Fork Session Up",
        L10nKey::SettingsAboutDesc1 => {
            "A terminal workbench for long-running work. Sessions outlive the window, remote hosts feel local, and coding agents report their status right in the sidebar."
        }
        L10nKey::SettingsDefaultTerminal => "Default terminal",
        L10nKey::SettingsDefaultTerminalDesc => {
            "Open Unix executables, SSH links and man pages in lyrel."
        }
        L10nKey::SettingsDefaultTerminalSet => "Set as Default Terminal",
        L10nKey::SettingsDefaultTerminalSetSuccess => {
            "lyrel is now the default handler for supported terminal files and links."
        }
        L10nKey::SettingsDefaultTerminalSetFailed => {
            "Could not set lyrel as the default terminal: {error}"
        }
        L10nKey::SettingsVersion => "Version",
        L10nKey::SettingsUpdates => "Updates",
        L10nKey::SettingsUpdateAndRelaunch => "Update and relaunch",
        L10nKey::SettingsUpdateViewRelease => "View release",
        L10nKey::SettingsUpdateChecking => "Checking for updates…",
        L10nKey::SettingsUpdateUpToDate => "You're running the latest version.",
        L10nKey::SettingsUpdateDownloadingPercent => "Downloading the update… {percent}% of {size}",
        L10nKey::SettingsUpdateDownloadingBytes => "Downloading the update… {received}",
        L10nKey::SettingsUpdateVerifying => "Verifying the downloaded update…",
        L10nKey::SettingsUpdateInstalling => "Relaunching with the update…",
        L10nKey::SettingsUpdateCheckNow => "Check now",
        L10nKey::SettingsUpdateCancel => "Cancel download",
        L10nKey::SettingsUpdateRetry => "Try again",
        L10nKey::SettingsUpdateDismiss => "Dismiss",
        L10nKey::SettingsUpdateDownloadManually => "Download manually",
        L10nKey::SettingsUpdateFailedTitle => "Updating to {version} failed.",
        L10nKey::SettingsUpdateReady => "Version {version} is downloaded and ready to install.",
        L10nKey::SettingsUpdateReadyNextLaunch => {
            "It will be installed the next time you start lyrel."
        }
        L10nKey::SettingsUpdateInstallNow => "Install and relaunch",
        L10nKey::SettingsUpdateDiscard => "Discard",
        L10nKey::SettingsAutoDownload => "Download updates in the background",
        L10nKey::SettingsAutoDownloadDesc => {
            "Download updates in the background so installing is just a restart. Nothing installs without asking."
        }
        L10nKey::SettingsDaemonStale => "lyrel server is still running {build}.",
        L10nKey::SettingsDaemonStaleDesc => {
            "lyrel was updated, but your panes still run on the old server. Restarting it loads the new build and ends everything running in them."
        }
        L10nKey::UpdateDialogTitle => "Update available",
        L10nKey::UpdateDialogDetail => {
            "lyrel {version} is available — you're on {current}. Installing restarts the app; lyrel server keeps running, so your panes survive."
        }
        L10nKey::UpdateDialogDetailWindows => {
            "lyrel {version} is available — you're on {current}. Installing restarts the app and lyrel server: processes in your panes are ended, and your tabs and layout come back with fresh shells."
        }
        L10nKey::UpdateDialogDetailManual => {
            "lyrel {version} is available — you're on {current}. {hint}"
        }
        L10nKey::UpdateDialogCannotSelfUpdate => "This installation cannot update itself.",
        L10nKey::UpdateDialogLater => "Later",
        L10nKey::UpdateDialogNextLaunch => "Install on Next Launch",
        L10nKey::UpdateDialogNeedsElevation => {
            "lyrel is installed for all users, so Windows asks for administrator approval once before installing. lyrel itself never runs elevated."
        }
        L10nKey::SettingsUpdateCheckFailed => "Could not check for updates: {error}",
        L10nKey::SettingsUpdatePrepareFailed => "Update failed: {error}",
        L10nKey::SettingsUpdateLaunchFailed => "Could not start the installer: {error}",
        L10nKey::SettingsUpdateUnsupportedMacos => {
            "This copy is not in a writable lyrel.app bundle, so it cannot replace itself. Move lyrel to Applications, or open the release page to update."
        }
        L10nKey::SettingsUpdateUnsupportedWindows => {
            "This copy is not a recognized Inno Setup or portable ZIP installation, so it cannot update itself. Open the release page to update it by hand."
        }
        L10nKey::SettingsUpdateWindowsAllUsers => {
            "lyrel is installed for all users, so replacing it needs administrator rights that lyrel will not ask for itself. Open the release page and run the installer to update."
        }
        L10nKey::SettingsUpdateMissingPackage => {
            "The release has no {name} package for this installation. Open the release page to choose another package."
        }
        L10nKey::SettingsUpdateMissingChecksums => {
            "The release has no checksums.txt, so lyrel refuses to install it automatically."
        }
        L10nKey::SettingsVersionAvailable => "Version {version} is available.",
        L10nKey::SettingsCheckUpdatesDesc => {
            "Opens the release page if lyrel can't update in place."
        }
        L10nKey::SettingsCheckUpdatesOnLaunch => "Check for updates on launch",
        L10nKey::SettingsCommandLine => "Command line",
        L10nKey::SettingsCommandLineDesc => {
            "Put the lyrel command on PATH. Applies on next launch."
        }
        L10nKey::SettingsInstallCliOnPath => "Install the lyrel command on PATH",
        L10nKey::SettingsServer => "lyrel server",
        L10nKey::SettingsServerDesc => "Keeps terminal sessions running in the background.",
        L10nKey::SettingsRestartServer => "Restart lyrel server…",
        L10nKey::SettingsAppHttpProxy => "Proxy for updates",
        L10nKey::SettingsAppHttpProxyDesc => {
            "For lyrel's update checks. Empty uses the system proxy."
        }
        L10nKey::SettingsAppHttpProxyInvalid => {
            "Not a valid proxy address — this value was not saved."
        }
        L10nKey::SettingsAgentOpencode => "OpenCode",
        L10nKey::SettingsAgentPi => "Pi",
        L10nKey::SettingsAgentOhMyPi => "Oh My Pi",
        L10nKey::SettingsAgentKimiCode => "Kimi Code",
        L10nKey::SettingsAgentHermes => "Hermes",
        L10nKey::SettingsSearchAboutKeywords => "version license credits build update check github",
        L10nKey::SettingsSearchAppHttpProxyKeywords => {
            "proxy http https socks socks5 clash v2ray network download update"
        }
        L10nKey::SettingsSearchAnsiColorsKeywords => "palette 16 terminal colours theme",
        L10nKey::SettingsSearchBackgroundImageKeywords => {
            "background image wallpaper picture photo backdrop theme"
        }
        L10nKey::SettingsSearchImageOpacityKeywords => {
            "background image opacity strength fade wallpaper"
        }
        L10nKey::SettingsSearchArgumentsKeywords => "shell flags login args",
        L10nKey::SettingsSearchBlurKeywords => {
            "transparency translucent frosted vibrancy window background"
        }
        L10nKey::SettingsSearchBoldFontKeywords => "typeface weight",
        L10nKey::SettingsSearchCommandLineToolKeywords => {
            "command line tool cli lyrel path shell command install symlink terminal iterm agent script"
        }
        L10nKey::SettingsSearchCopyOnSelectKeywords => "clipboard selection yank mouse",
        L10nKey::SettingsSearchCursorBlinkKeywords => "caret blinking flash",
        L10nKey::SettingsSearchCursorShapeKeywords => "caret block bar underline beam",
        L10nKey::SettingsSearchPromptCursorShapeKeywords => {
            "prompt cursor shape caret block bar underline beam shell integration"
        }
        L10nKey::SettingsSearchCustomThemesKeywords => {
            "theme duplicate edit colors folder yaml import background image wallpaper"
        }
        L10nKey::SettingsSearchDetectUrlsKeywords => "links hyperlink clickable open",
        L10nKey::SettingsSearchDimInactivePanesKeywords => {
            "fade unfocused inactive split pane focus opacity highlight active dimming"
        }
        L10nKey::SettingsSearchFocusFollowsMouseKeywords => "pane hover activate",
        L10nKey::SettingsSearchFontFamilyKeywords => "typeface monospace typography",
        L10nKey::SettingsSearchFontLigaturesKeywords => "typography glyph fira",
        L10nKey::SettingsSearchFontThickenKeywords => {
            "font smoothing thicken bold weight thin dilation antialiasing AppleFontSmoothing"
        }
        L10nKey::SettingsSearchFontSizeKeywords => "typography text bigger smaller zoom",
        L10nKey::SettingsSearchForwardSshLoopbackLinksKeywords => {
            "ssh remote port tunnel localhost forward links ports autoforward detect"
        }
        L10nKey::SettingsSearchHideMouseWhileTypingKeywords => "cursor pointer autohide",
        L10nKey::SettingsSearchHistorySearchKeywords => {
            "ctrl-r reverse search fuzzy history recall fzf prompt"
        }
        L10nKey::SettingsSearchHostsKeywords => {
            "ssh host connection saved profile import ssh_config manage add edit quick connect"
        }
        L10nKey::SettingsSearchItalicFontKeywords => "typeface oblique",
        L10nKey::SettingsSearchKeybindingsKeywords => {
            "shortcut hotkey keyboard binding chord tmux preset rebind prefix"
        }
        L10nKey::SettingsSearchKeybindingsTitle => "Keyboard shortcuts",
        L10nKey::SettingsSearchLineHeightKeywords => "typography leading spacing",
        L10nKey::SettingsSearchUiFontFamilyKeywords => {
            "interface font family ui typeface typography chrome sidebar tab"
        }
        L10nKey::SettingsSearchNewTabPositionKeywords => "tabs order end after current",
        L10nKey::SettingsSearchConfirmCloseKeywords => {
            "confirm close closing tab pane ask prompt warn busy running idle always never"
        }
        L10nKey::SettingsSearchNotifyOnCommandFinishKeywords => {
            "notification alert done osc desktop banner long command"
        }
        L10nKey::SettingsSearchNotifyThresholdKeywords => {
            "notification alert seconds duration long command delay"
        }
        L10nKey::SettingsSearchOpacityKeywords => {
            "transparency translucent see through window alpha"
        }
        L10nKey::SettingsSearchOpenFilesWithKeywords => {
            "links file editor command external app path line column"
        }
        L10nKey::SettingsSearchOpencodeKeywords => "agent integration plugin install opencode",
        L10nKey::SettingsSearchOptionAsMetaKeywords => {
            "alt keyboard modifier escape macos option meta option acts as meta"
        }
        L10nKey::SettingsSearchOhMyPiKeywords => "agent integration extension install omp oh my pi",
        L10nKey::SettingsSearchKimiCodeKeywords => {
            "agent integration hooks install kimi code kimi-code moonshot"
        }
        L10nKey::SettingsSearchHermesKeywords => "agent integration plugin observer install hermes",
        L10nKey::SettingsSearchPiKeywords => "agent integration extension install pi",
        L10nKey::SettingsSearchPortForwardingKeywords => {
            "ssh tunnel local remote dynamic socks forward rule"
        }
        L10nKey::SettingsSearchProgramKeywords => {
            "shell binary zsh bash fish nu nushell pwsh powershell executable launch"
        }
        L10nKey::SettingsSearchRememberWindowSizeKeywords => {
            "window size position bounds geometry launch startup remember"
        }
        L10nKey::SettingsSearchReportMouseToAppsKeywords => {
            "mouse reporting vim tmux click scroll shift passthrough"
        }
        L10nKey::SettingsSearchRestoreLastLayoutKeywords => {
            "restore session previous tabs splits reopen launch startup layout"
        }
        L10nKey::SettingsSearchScrollSpeedKeywords => "mouse wheel multiplier scrolling",
        L10nKey::SettingsSearchSmoothScrollKeywords => {
            "smooth animation ease wheel notch trackpad scrolling"
        }
        L10nKey::SettingsSearchCheckUpdatesOnLaunchKeywords => {
            "update check launch startup automatic release"
        }
        L10nKey::SettingsSearchAutoDownloadKeywords => {
            "update download background install metered connection"
        }
        L10nKey::SettingsSearchScrollbackKeywords => "history buffer lines scroll",
        L10nKey::SettingsSearchShowTrayIconKeywords => {
            "tray menu bar status item agent attention system icon"
        }
        L10nKey::SettingsSearchSidebarGroupingKeywords => {
            "tabs group grouping auto repo repository git ssh host pinned pin ungrouped header sidebar flat folder"
        }
        L10nKey::SettingsSearchSmartSelectionKeywords => {
            "double click word url path select semantic bracket email"
        }
        L10nKey::SettingsSearchStartInKeywords => {
            "cwd working directory start folder path home inherit custom"
        }
        L10nKey::SettingsSearchSyncWithSystemKeywords => {
            "theme dark light auto follow os appearance mode"
        }
        L10nKey::SettingsSearchLegiblePaletteKeywords => {
            "legible contrast bright palette psreadline parameter readable"
        }
        L10nKey::SettingsSearchPromptEditorKeywords => {
            "prompt editor native shell input line editor zle readline fish keybindings ime paste"
        }
        L10nKey::SettingsSearchTabCompletionKeywords => {
            "complete completion menu suggestions tab prompt"
        }
        L10nKey::SettingsSearchTerminalBellKeywords => {
            "bell audible visual flash sound silence beep both ^g"
        }
        L10nKey::SettingsSearchThemeKeywords => {
            "appearance color colours scheme dark light palette background foreground accent sync system os auto follow"
        }
        L10nKey::SettingsSearchTrimTrailingSpacesKeywords => "clipboard whitespace copy",
        L10nKey::SettingsSearchVerifyHostKeysKeywords => {
            "ssh security known_hosts fingerprint mitm host key verification"
        }
        L10nKey::SettingsSearchWarnBeforeClosingKeywords => {
            "ssh confirm close tab pane live session security"
        }
        L10nKey::SettingsSearchStartupWindowKeywords => "launch open maximized fullscreen normal",
        L10nKey::SwitcherNoMatch => "No workspace or machine matches.",
        L10nKey::AddSshHost => "Add SSH Host…",
        L10nKey::RestartServer => "Restart lyrel server",
        L10nKey::OtherMachines => "Other Machines",
        L10nKey::Ok => "OK",
        L10nKey::SftpNoTransfers => "No transfers yet.",
        L10nKey::SftpPanelTitleFiles => "Files",
        L10nKey::SftpTooltipRefresh => "Refresh",
        L10nKey::SftpTooltipMore => "More",
        L10nKey::SftpMenuNewFolder => "New Folder",
        L10nKey::SftpMenuNewFile => "New File",
        L10nKey::SftpMenuUpload => "Upload…",
        L10nKey::SftpMenuGotoShellCwd => "Go to Shell Directory",
        L10nKey::SftpMenuHideTransferHistory => "Hide Transfer History",
        L10nKey::SftpMenuTransferHistory => "Transfer History",
        L10nKey::SftpEditNewFolder => "New folder",
        L10nKey::SftpEditNewFile => "New file",
        L10nKey::SftpEditRename => "Rename",
        L10nKey::SftpEditPermissions => "Permissions · {mode}",
        L10nKey::SftpLoading => "Loading…",
        L10nKey::SftpEmptyDirectory => "Empty directory.",
        L10nKey::SftpContextOpen => "Open",
        L10nKey::SftpContextEdit => "Edit",
        L10nKey::SftpContextFollowSymlink => "Follow Symlink",
        L10nKey::SftpContextRename => "Rename",
        L10nKey::SftpContextChmod => "chmod…",
        L10nKey::SftpTransferSummaryRunning => "{count} transferring · {pct}%",
        L10nKey::SftpTransferSummaryFailed => "{count} failed",
        L10nKey::SftpTransferSummaryIdle => "Transfers",
        L10nKey::SftpTransferProgress => "{done} / {total} ({pct}%)",
        L10nKey::SftpTransferDone => "done",
        L10nKey::SftpTransferCancelled => "cancelled",
        L10nKey::SftpTransferError => "error",
        L10nKey::SftpTransferListFailed => "Could not check transfers: {error}",
        L10nKey::SftpPasteUploadFailed => "Could not upload {name} to {host}: {error}",
        L10nKey::LinkFileOpenFailed => "Could not open {path}: {error}",
        L10nKey::ForwardDisconnected => "Disconnected",
        L10nKey::ForwardDisconnectedFrom => "Disconnected from {host}",
        L10nKey::SshEditProfile => "Edit connection…",
        L10nKey::ForwardTooltipAdd => "Add forward",
        L10nKey::ForwardTooltipRemove => "Remove",
        L10nKey::ForwardTooltipTurnOn => "Turn on",
        L10nKey::ForwardTooltipTurnOff => "Turn off — the rule is kept",
        L10nKey::ForwardSwitchFailed => "Couldn't switch the forward — {error}",
        L10nKey::SettingsFwdEnabled => "Open this rule with the connection",
        L10nKey::ForwardLocal => "Local",
        L10nKey::ForwardRemote => "Remote",
        L10nKey::ForwardDynamic => "Dynamic",
        L10nKey::ForwardBindLabel => "bind",
        L10nKey::ForwardToLabel => "to",
        L10nKey::ForwardSocksLabel => "SOCKS",
        L10nKey::ForwardAdd => "Add",
        L10nKey::ForwardPortLabel => "Remote port",
        L10nKey::ForwardPortHere => "opens at localhost:{port}",
        L10nKey::ForwardNeedsPort => "A port is a number from 1 to 65535.",
        L10nKey::ForwardAdvancedToggle => "Advanced",
        L10nKey::ForwardSimpleToggle => "Simple",
        L10nKey::ForwardRequestFailed => "Could not reach the session — nothing changed.",
        L10nKey::FileTreePlaceholderFileName => "file name",
        L10nKey::FileTreePlaceholderFolderName => "folder name",
        L10nKey::FileTreePlaceholderNewName => "new name",
        L10nKey::FileTreeDeleteTitle => "Delete \"{name}\"?",
        L10nKey::FileTreeDeleteFolderBody => {
            "The folder and everything inside it will be deleted. This can't be undone."
        }
        L10nKey::FileTreeDeleteFileBody => "This can't be undone.",
        L10nKey::SftpDeleteFolderBody => {
            "The folder and everything inside it will be deleted on {host}. There is no trash on the far side."
        }
        L10nKey::SftpDeleteFileBody => {
            "The file will be deleted on {host}. There is no trash on the far side."
        }
        L10nKey::FileTreeDeleteFailed => "Could not delete {name}",
        L10nKey::FileTreeCreateFailed => "Could not create {name}",
        L10nKey::FileTreeRenameFailed => "Could not rename {name}",
        L10nKey::FileTreeDownloadFailed => "Could not download {name}",
        L10nKey::FileTreeDownloaded => "Downloaded to {path}",
        L10nKey::FileTreeDownloadTooLarge => {
            "Larger than {limit} MB — fetch it with scp or rsync instead."
        }
        L10nKey::FileTreeContextOpen => "Open",
        L10nKey::FileTreeContextCdHere => "cd Here",
        L10nKey::FileTreeContextPinAsGroup => "Pin as Group",
        L10nKey::FileTreeContextInsertPath => "Insert Path in Terminal",
        L10nKey::FileTreeContextAttachAgent => "Attach to Agent",
        L10nKey::FileTreeContextNewFile => "New File",
        L10nKey::FileTreeContextNewFolder => "New Folder",
        L10nKey::FileTreeContextRename => "Rename",
        L10nKey::FileTreeContextCopyPath => "Copy Path",
        L10nKey::FileTreeContextHideDotfiles => "Hide Dotfiles",
        L10nKey::MarkdownImageChooseLocation => "Choose a save location before inserting the image",
        L10nKey::MarkdownImageWriting => "Writing image to document assets…",
        L10nKey::MarkdownImageWriteFailed => "Could not write image to document assets",
        L10nKey::MarkdownImageSelectionChanged => {
            "The image was saved, but the selected text changed. No link was inserted."
        }
        L10nKey::FileTreeContextShowDotfiles => "Show Dotfiles",
        L10nKey::FileDropIntoItself => "A folder cannot be copied into itself.",
        L10nKey::FileDropNotHere => "Not on this machine.",
        L10nKey::FileDropNameTaken => "Another item in the same drop already has that name.",
        L10nKey::FileDropTooDeep => "Nested more than {n} folders deep.",
        L10nKey::FileDropTooLarge => "Larger than {limit} MB — send it over SFTP instead.",
        L10nKey::FileDropNoWorkingName => "No free name beside it to copy onto first.",
        L10nKey::FileDropLeftAside => {
            "The copy could not be put in place; what was there is now named \"{name}\" in the same folder."
        }
        L10nKey::FileDropReplaceTitle => "Replace \"{name}\"?",
        L10nKey::FileDropReplaceManyTitle => "Replace {n} items?",
        L10nKey::FileDropReplaceBody => {
            "This folder already has something by that name. Replacing it cannot be undone."
        }
        L10nKey::FileDropReplace => "Replace",
        L10nKey::FileDropFailed => "Could not copy {name}",
        L10nKey::FileDropFailedMany => "Could not copy {name}, and {n} more failed",
        L10nKey::SshPromptNewKey => "new {fingerprint}",
        L10nKey::SshPromptOldKey => "old {old_fingerprint}",
        L10nKey::SshPromptHostKeyNewAlgorithm => {
            "You already know this host by a {previous_algorithm} key. This is a new \
             {algorithm} key, not a replacement for that one."
        }
        L10nKey::SshPromptTypeYesToOverride => "Type \"yes\" to enable Override.",
        L10nKey::EditorCantOpen => "Could not open {path}: {e}",
        L10nKey::EditorCantRead => "Could not read {path}: {e}",
        L10nKey::EditorNotUtf8 => "\"{path}\" is not valid UTF-8",
        L10nKey::EditorSaveFailed => "Could not save {name}",
        L10nKey::EditorUnsavedChanges => "\"{name}\" has unsaved changes",
        L10nKey::EditorDiscard => "Discard",
        L10nKey::EditorNoFileOpen => "No file open",
        L10nKey::EditorStripSearch => "Search {n} open files",
        L10nKey::EditorStripAllFiles => "All open files",
        L10nKey::EditorStripHidden => "Hidden · {n}",
        L10nKey::EditorStripInBar => "In tab bar",
        L10nKey::EditorStripNoMatch => "No open files match",
        L10nKey::EditorStripCloseSaved => "Close saved",
        L10nKey::EditorStripCloseOthers => "Close others",
        L10nKey::EditorBackToTerminal => "Back to Terminal (Esc)",
        L10nKey::EditorLnCol => "Ln {line}, Col {column}",
        L10nKey::EditorToggleLineEnding => "Switch line endings between LF and CRLF",
        L10nKey::StatusRemoteHost => "Remote host {id}",
        L10nKey::EditorMarkdownLive => "Live editing",
        L10nKey::EditorMarkdownSource => "Source",
        L10nKey::EditorMarkdownLiveLabel => "Markdown live editor",
        L10nKey::EditorMarkdownSourceLabel => "Markdown source",
        L10nKey::EditorMarkdownTaskLabel => "Toggle task: {text}",
        L10nKey::EditorMarkdownFormulaLabel => "Formula: {tex}",
        L10nKey::EditorMdxEditName => "Edit component name",
        L10nKey::EditorMdxEditProperty => "Edit property: {name}",
        L10nKey::EditorMdxEditChildren => "Edit component children",
        L10nKey::EditorCardEditName => "Edit card component name",
        L10nKey::EditorCardFollowLabel => "Open card destination: {target}",
        L10nKey::EditorCardAddProperty => "Add card property: {name}",
        L10nKey::EditorCardEditProperty => "Edit card property: {name}",
        L10nKey::EditorCardEditGroupLabel => "Edit card group children",
        L10nKey::EditorAccordionCollapse => "Collapse accordion: {title}",
        L10nKey::EditorAccordionExpand => "Expand accordion: {title}",
        L10nKey::EditorMarkdownSourceRegion => {
            "Source · structure not yet adapted for live editing"
        }
        L10nKey::EditorMdxGeneric => "Generic JSX structure · not runtime output",
        L10nKey::EditorMdxName => "Component name",
        L10nKey::EditorMdxProperty => "Property",
        L10nKey::EditorMdxDynamic => "Dynamic source · never executed",
        L10nKey::EditorMdxChildren => "Children · continuous Markdown",
        L10nKey::EditorMdxClosing => "End component",
        L10nKey::EditorMdxParseError => "Invalid MDX · edit the retained source to recover",
        L10nKey::EditorCard => "Card",
        L10nKey::EditorCardGroup => "Card group",
        L10nKey::EditorCardGroupOrder => "Cards remain in document reading order",
        L10nKey::EditorCardTitle => "Edit title",
        L10nKey::EditorCardDestination => "Edit destination",
        L10nKey::EditorCardIcon => "Edit icon",
        L10nKey::EditorCardDescription => "Edit description",
        L10nKey::EditorCardColumns => "Edit columns",
        L10nKey::EditorCardFollow => "Open destination",
        L10nKey::EditorCardEditContent => "Edit card content",
        L10nKey::EditorCardEditGroup => "Edit cards",
        L10nKey::EditorMdxStepsNumber => "Step {number}",
        L10nKey::EditorMdxStepsSequence => "Procedure · {count} steps",
        L10nKey::EditorMdxStepsSequenceSingular => "Procedure · {count} step",
        L10nKey::EditorMdxStepsEditTitle => "Edit step title",
        L10nKey::EditorMdxStepsEditSource => "Edit step source",
        L10nKey::EditorMdxStepsEditContainerSource => "Edit procedure source",
        L10nKey::EditorMdxStepsEndStep => "End step",
        L10nKey::EditorMdxStepsEndSequence => "End procedure",
        L10nKey::EditorCalloutNote => "Note",
        L10nKey::EditorCalloutTip => "Tip",
        L10nKey::EditorCalloutInfo => "Information",
        L10nKey::EditorCalloutWarning => "Warning",
        L10nKey::EditorCalloutEditSource => "Edit callout source",
        L10nKey::EditorCalloutReturnContent => "Return to callout content",
        L10nKey::EditorMdxAccordionsGroup => "Accordion group",
        L10nKey::EditorMdxAccordionsSection => "Accordion",
        L10nKey::EditorMdxAccordionsExpand => "Expand accordion",
        L10nKey::EditorMdxAccordionsCollapse => "Collapse accordion",
        L10nKey::EditorMdxTabsGroup => "Tabs",
        L10nKey::EditorMdxTabsUntitled => "Untitled tab",
        L10nKey::EditorMdxTabsEmpty => "No Tab children. Edit children to add a tab.",
        L10nKey::EditorMdxTabsKeyboard => "In content: Alt+Page Up / Page Down switches tabs",
        L10nKey::EditorMarkdownCodeRegion => "Code source",
        L10nKey::EditorMarkdownMermaidSource => "Mermaid source",
        L10nKey::EditorMarkdownMermaidLoading => "Rendering diagram…",
        L10nKey::EditorMarkdownMermaidEdit => "Edit diagram source",
        L10nKey::EditorMarkdownMermaidError => {
            "Diagram could not render. Source is retained and editable."
        }
        L10nKey::EditorMarkdownMermaidEmpty => {
            "No Mermaid diagram detected. Enter a diagram in the retained source."
        }
        L10nKey::EditorMarkdownMermaidExternal => {
            "External diagram resources are not allowed. Use self-contained labels and shapes."
        }
        L10nKey::EditorMarkdownMermaidRasterLimit => {
            "Diagram exceeds the native image limit (4096 pixels per side or 8 million pixels). Simplify or split the diagram."
        }
        L10nKey::EditorMarkdownMetadataRegion => "Metadata source",
        L10nKey::EditorMarkdownHtmlRegion => "HTML source · not executed or rendered as HTML/CSS",
        L10nKey::EditorMarkdownMathLoading => "Rendering formula",
        L10nKey::EditorMarkdownMathError => {
            "Couldn’t render formula. Click to edit its source. Reason:"
        }
        L10nKey::EditorLoadImages => "Load document images",
        L10nKey::EditorImageUnloaded => "Image not loaded",
        L10nKey::EditorImageLoading => "Loading image",
        L10nKey::EditorImageLoaded => "Image loaded",
        L10nKey::EditorImageFailed => "Couldn’t load image",
        L10nKey::EditorTableCommands => "Table…",
        L10nKey::EditorTableInsertRowBefore => "Table: Insert row above",
        L10nKey::EditorTableInsertRowAfter => "Table: Insert row below",
        L10nKey::EditorTableDeleteRow => "Table: Delete row",
        L10nKey::EditorTableInsertColumnBefore => "Table: Insert column before",
        L10nKey::EditorTableInsertColumnAfter => "Table: Insert column after",
        L10nKey::EditorTableDeleteColumn => "Table: Delete column",
        L10nKey::EditorTableAlignDefault => "Table: Default alignment",
        L10nKey::EditorTableAlignLeft => "Table: Align left",
        L10nKey::EditorTableAlignCenter => "Table: Align center",
        L10nKey::EditorTableAlignRight => "Table: Align right",
        L10nKey::MarkdownEditLinkDestination => "Edit link destination",
        L10nKey::MarkdownEditReferenceDestination => "Edit reference destination",
        L10nKey::MarkdownFollowLink => "Open link",
        L10nKey::MarkdownLinkOpenFailed => "Couldn’t open link: {reason}",
        L10nKey::MarkdownLinkInvalidDestination => "The destination is not a valid path or URL",
        L10nKey::MarkdownLinkUnsupportedScheme => {
            "This link scheme is not supported on the document’s Host"
        }
        L10nKey::MarkdownLinkSaveFirst => "Save the document before opening a relative link",
        L10nKey::MarkdownLinkRemotePath => "A remote destination must use a POSIX path",
        L10nKey::EditorWrapOn => "Wrap: on",
        L10nKey::EditorWrapOff => "Wrap: off",
        L10nKey::EditorFileTooLarge => "\"{path}\" is too large for the editor ({size} MB)",
        L10nKey::EditorBinaryFile => "\"{path}\" looks like a binary file",
        L10nKey::EditorUntitled => "Untitled-{n}",
        L10nKey::EditorUnsavedChangesMany => "{count} files have unsaved changes",
        L10nKey::EditorSaveAll => "Save All",
        L10nKey::EditorSaveConflictTitle => "\"{name}\" changed on disk",
        L10nKey::EditorSaveConflictBody => {
            "Another program changed it after it was opened here. Overwriting replaces those changes with yours."
        }
        L10nKey::EditorOverwrite => "Overwrite",
        L10nKey::EditorEncodeFailedTitle => "Can't save \"{name}\" as {encoding}",
        L10nKey::EditorEncodeFailedBody => {
            "It contains \"{ch}\", which {encoding} can't represent. Save it as UTF-8 instead?"
        }
        L10nKey::EditorSaveAsUtf8 => "Save as UTF-8",
        L10nKey::EditorAlreadyOpen => "\"{path}\" is already open in the editor",
        L10nKey::EditorGoToLine => "Go to line",
        L10nKey::EditorGoToLineAction => "Go to Line…",
        L10nKey::EditorGoToMatchingBracket => "Go to Matching Bracket",
        L10nKey::EditorToggleComment => "Toggle Comment",
        L10nKey::EditorMoveLineUp => "Move Line Up",
        L10nKey::EditorMoveLineDown => "Move Line Down",
        L10nKey::EditorDuplicateLine => "Duplicate Line",
        L10nKey::EditorDeleteLine => "Delete Line",
        L10nKey::EditorCopyRelativePath => "Copy Relative Path",
        L10nKey::EditorGitNextChange => "Go to Next Change",
        L10nKey::EditorGitPrevChange => "Go to Previous Change",
        L10nKey::EditorGitRevertChange => "Revert Change",
        L10nKey::EditorGitToggleGutter => "Toggle Git Change Markers",
        L10nKey::EditorGitPeekRevert => "Revert",
        L10nKey::EditorGitPeekSummary => "−{removed} +{added} lines against the staged version",
        L10nKey::EditorGitPeekAddedOnly => {
            "These lines are new: the staged version has nothing here."
        }
        L10nKey::EditorGitPeekChange => "Peek Change",
        L10nKey::EditorGitPeekKeys => "Enter reverts · Esc closes",
        L10nKey::EditorProblemsTitle => "Problems",
        L10nKey::EditorProblemsToggle => "Toggle Problems",
        L10nKey::EditorProblemsNone => "No problems in the open files.",
        L10nKey::EditorProblemsMore => "…and {n} more",
        L10nKey::SettingsEditor => "Editor",
        L10nKey::SettingsEditorGitGutter => "Git change markers",
        L10nKey::SettingsEditorGitGutterDesc => {
            "Mark lines that differ from the staged version beside the line numbers and on the scrollbar."
        }
        L10nKey::SettingsEditorLsp => "Language servers",
        L10nKey::SettingsEditorLspDesc => {
            "Start a language server for files it knows, for errors, completion and go to definition. Only files on this machine."
        }
        L10nKey::SettingsEditorSoftWrap => "Wrap long lines",
        L10nKey::SettingsEditorSoftWrapDesc => {
            "Open files with soft wrap on. The status bar's Wrap button changes it for one file."
        }
        L10nKey::SettingsSearchEditorGitGutterKeywords => {
            "git gutter diff changes markers staged index scm vcs modified added deleted"
        }
        L10nKey::SettingsSearchEditorLspKeywords => {
            "lsp language server diagnostics errors warnings completion rust-analyzer definition"
        }
        L10nKey::SettingsSearchEditorSoftWrapKeywords => "wrap soft wrap long lines editor",
        L10nKey::EditorGoToLinePlaceholder => "Line, or line:column (1–{total})",
        L10nKey::EditorGoToSymbolAction => "Go to Symbol in Editor…",
        L10nKey::EditorNavigateBack => "Go Back",
        L10nKey::EditorNavigateForward => "Go Forward",
        L10nKey::EditorSplitRight => "Split Editor Right",
        L10nKey::EditorFocusLeftGroup => "Focus Left Editor Group",
        L10nKey::EditorFocusRightGroup => "Focus Right Editor Group",
        L10nKey::EditorSplitSameFile => "Open in the other group — click to edit it here",
        L10nKey::CmdEditorGoToSymbol => "Editor: Go to Symbol…",
        L10nKey::SearchHeadingReferences => "References",
        L10nKey::SearchSectionThisFile => "In This File",
        L10nKey::SearchSectionProject => "Project",
        L10nKey::SearchHeadingDefinitions => "Definitions",
        L10nKey::CmdEditorGoBack => "Editor: Go Back",
        L10nKey::CmdEditorGoForward => "Editor: Go Forward",
        L10nKey::CmdEditorSplitRight => "Editor: Split Right",
        L10nKey::EditorSaveAs => "Save as",
        L10nKey::EditorSaveAsAction => "Save As…",
        L10nKey::EditorSaveAsPlaceholder => "Full path to save to",
        L10nKey::EditorReplaceExisting => "\"{path}\" already exists. Replace it?",
        L10nKey::EditorReplace => "Replace",
        L10nKey::EditorNewFile => "New File",
        L10nKey::EditorOrphanAdopted => "Unsaved \"{name}\" was moved here from a tab that closed",
        L10nKey::EditorFileDeletedOnDisk => "This file was deleted on disk",
        L10nKey::EditorIndentSpaces => "Spaces: {n}",
        L10nKey::EditorIndentTabs => "Tab Size: {n}",
        L10nKey::LspGoToDefinition => "Go to Definition",
        L10nKey::LspQuickFix => "Quick Fix…",
        L10nKey::LspFormatDocument => "Format Document",
        L10nKey::LspRenameSymbol => "Rename symbol",
        L10nKey::LspRenameSymbolAction => "Rename Symbol…",
        L10nKey::LspRenamePlaceholder => "New name for {name}",
        L10nKey::LspRenameFailed => "Could not rename {name}",
        L10nKey::LspServerMissing => "{name} not installed",
        L10nKey::LspServerStarting => "{name} starting…",
        L10nKey::LspServerDown => "{name} stopped",
        L10nKey::LspProblemsTooltip => "{errors} errors, {warnings} warnings",
        L10nKey::SearchTabLocations => "Locations",
        L10nKey::SearchPlaceholderLocations => "Filter the places found…",
        L10nKey::SearchLocationsNone => "Nothing found",
        L10nKey::LspFindReferences => "Find All References",
        L10nKey::PanelInfoTitle => "Info",
        L10nKey::PanelChangesTitle => "Changes",
        L10nKey::PanelScmTitle => "Changes",
        L10nKey::PanelFilesTitle => "Files",
        L10nKey::PanelSearchTitle => "Search",
        L10nKey::PanelGitHubTitle => "GitHub",
        L10nKey::PanelNoSession => "No active session.",
        L10nKey::PanelNoSessionHint => {
            "Open a tab to see its shell, directory, and processes here."
        }
        L10nKey::PanelNoWorkingDirectory => "No working directory.",
        L10nKey::PanelNoWorkingDirectoryHint => "This pane has not reported one yet.",
        L10nKey::PanelLoading => "Loading…",
        L10nKey::PanelNotAGitRepo => "Not a git repository.",
        L10nKey::PanelNotAGitRepoHint => "cd into one and this tab lists its uncommitted changes.",
        L10nKey::PanelNoChanges => "No uncommitted changes.",
        L10nKey::PanelNoChangesHint => "The working tree is clean.",
        L10nKey::PanelSessionSubtitle => "Session",
        L10nKey::PanelProcessesSubtitle => "Processes",
        L10nKey::PanelProcessesTotal => "Total",
        L10nKey::PanelPortsSubtitle => "Ports",
        L10nKey::PanelPortsUnsupported => "That machine's lyrel-server is too old to list ports.",
        L10nKey::PanelPortsProbeFailed => "Couldn't check what this pane is listening on.",
        L10nKey::PanelPortsRestricted => {
            "Something here runs as another user, whose ports aren't visible."
        }
        L10nKey::PanelPortsEmpty => "No forwarded ports",
        L10nKey::PanelLatency => "latency",
        L10nKey::PortAutoForwarded => "Remote :{port} is now http://localhost:{local}",
        L10nKey::PanelCwd => "cwd",
        L10nKey::PanelShell => "shell",
        L10nKey::PanelSsh => "ssh",
        L10nKey::PanelBranch => "branch",
        L10nKey::PanelChangesRow => "changes",
        L10nKey::PanelAgentWorking => "working",
        L10nKey::PanelAgentWaiting => "waiting",
        L10nKey::PanelAgentDone => "done",
        L10nKey::PanelRevealInFinder => "Reveal in Finder",
        L10nKey::PanelOpenFolder => "Open Folder",
        L10nKey::PanelOpenInBrowser => "Open in Browser",
        L10nKey::ScmGroupMerge => "Merge Changes",
        L10nKey::ScmGroupStaged => "Staged Changes",
        L10nKey::ScmGroupChanges => "Changes",
        L10nKey::ScmGroupUntracked => "Untracked",
        L10nKey::ScmCommitPlaceholder => "Say what changed…",
        L10nKey::ScmCommitButton => "Commit",
        L10nKey::ScmCommitAllButton => "Commit All",
        L10nKey::ScmCommitAmendButton => "Commit (Amend)",
        L10nKey::ScmCommitAndPush => "Commit & Push",
        L10nKey::ScmCommitAndSync => "Commit & Sync",
        L10nKey::ScmAmendLastCommit => "Amend Last Commit",
        L10nKey::ScmStashAll => "Stash All",
        L10nKey::ScmNothingToCommit => "Nothing to commit",
        L10nKey::ScmNetworkBusy => "Another network operation is still running for this repository",
        L10nKey::ScmCommitNeedsMessage => "Write a commit message first",
        L10nKey::ScmDetailFilesFailed => "The file list could not be read",
        L10nKey::ScmTimeNow => "now",
        L10nKey::ScmTimeMinutes => "{n}m",
        L10nKey::ScmTimeHours => "{n}h",
        L10nKey::ScmTimeDays => "{n}d",
        L10nKey::ScmTimeMonths => "{n}mo",
        L10nKey::ScmTimeYears => "{n}y",
        L10nKey::ScmResetHardConfirm => {
            "Reset the branch to this commit? Commits after it fall off the branch, \
             and uncommitted changes are discarded."
        }
        L10nKey::ScmReset => "Reset",
        L10nKey::ScmChipStaged => "STAGED",
        L10nKey::ScmStage => "Stage Changes",
        L10nKey::ScmStageAll => "Stage All Changes",
        L10nKey::ScmUnstage => "Unstage Changes",
        L10nKey::ScmUnstageAll => "Unstage All Changes",
        L10nKey::ScmDiscard => "Discard Changes",
        L10nKey::ScmDiscardAll => "Discard All Changes",
        L10nKey::ScmDiscardConfirm => "Discard changes to {path}? This cannot be undone.",
        L10nKey::ScmOpenConflict => "Resolve Conflict",
        L10nKey::ScmMarkResolved => "Mark as Resolved",
        L10nKey::ScmUnrepresentablePath => {
            "This path is not valid UTF-8, so git cannot be asked about it — read only."
        }
        L10nKey::ScmPublishBranch => "Publish Branch",
        L10nKey::ScmDetached => "detached",
        L10nKey::ScmPushDetached => "Detached HEAD — check out a branch to push",
        L10nKey::ScmPushNoCommits => "No commits to push yet",
        L10nKey::ScmAmendBadge => "amend",
        L10nKey::ScmSync => "Sync Changes",
        L10nKey::ScmPush => "Push",
        L10nKey::ScmPull => "Pull",
        L10nKey::ScmFetch => "Fetch",
        L10nKey::ScmCheckoutBranch => "Checkout to…",
        L10nKey::ScmCreateBranch => "Create Branch…",
        L10nKey::ScmGraphTitle => "History",
        L10nKey::ScmGraphLoadMore => "Load more",
        L10nKey::ScmGraphFilterPlaceholder => "Filter commits…",
        L10nKey::ScmGraphAllBranches => "All Branches",
        L10nKey::ScmGraphEmpty => "No commits yet",
        L10nKey::ScmGraphCurrentBranch => "Current Branch",
        L10nKey::ScmCheckoutCommit => "Checkout Commit",
        L10nKey::ScmCreateBranchHere => "Create Branch Here…",
        L10nKey::ScmResetSoft => "Reset (Soft)",
        L10nKey::ScmResetMixed => "Reset (Mixed)",
        L10nKey::ScmResetHard => "Reset (Hard)",
        L10nKey::ScmCopyCommitSha => "Copy Commit SHA",
        L10nKey::ScmCherryPick => "Cherry Pick",
        L10nKey::ScmRevertCommit => "Revert Commit",
        L10nKey::ScmRefresh => "Refresh",
        L10nKey::ScmBackToChanges => "Back",
        L10nKey::ScmCommitParents => "Parents",
        L10nKey::ScmShowMore => "Show more",
        L10nKey::ScmShowLess => "Show less",
        L10nKey::ScmCommitNotFound => "This commit is not in this repository.",
        L10nKey::ScmFilterChanges => "Filter changed files…",
        L10nKey::ScmViewAsTree => "View as Tree",
        L10nKey::ScmViewAsList => "View as List",
        L10nKey::ScmNoMatchingChanges => "No changed files match the filter.",
        L10nKey::ScmTooManyChanges => "Showing the first {shown} of {total} changes.",
        L10nKey::ScmOpenChanges => "Open Changes",
        L10nKey::ScmDiscardAllConfirm => {
            "Discard all unstaged and untracked changes? Staged changes are kept. \
             This cannot be undone."
        }
        L10nKey::ScmAmendConfirm => {
            "Amend the last commit? It will be replaced by a new one, so anyone who already has it has to reconcile."
        }
        L10nKey::ScmOpMerge => "merging",
        L10nKey::ScmOpRebase => "rebasing",
        L10nKey::ScmOpCherryPick => "cherry-picking",
        L10nKey::ScmOpRevert => "reverting",
        L10nKey::ScmOpBisect => "bisecting",
        L10nKey::ScmOpAm => "applying",
        L10nKey::ScmSwitchRepository => "Switch Repository",
        L10nKey::WindowStop => "Stop",
        L10nKey::WindowDelete => "Delete",
        L10nKey::WindowThisWorkspace => "this workspace",
        L10nKey::WindowConfirmTitle => "{verb} Workspace \"{name}\"?",
        L10nKey::WindowStopUnreachable => {
            "Its machine could not be reached. Any shells still running there will be ended."
        }
        L10nKey::WindowDeleteUnreachable => {
            "Its machine could not be reached. Any shells still running there will be ended, and the layout forgotten."
        }
        L10nKey::WindowStopShells => "{count} running shells will be ended.",
        L10nKey::WindowDeleteShells => {
            "{count} running shells will be ended and the layout forgotten."
        }
        L10nKey::DiffReading => "Reading diff…",
        L10nKey::DiffNotARepo => "Not a git repository",
        L10nKey::DiffReadFailed => {
            "Could not read the working-tree diff — retrying on the next refresh."
        }
        L10nKey::DiffWorkingTreeClean => "Working tree clean",
        L10nKey::DiffCloseTooltip => "Close Diff (Esc)",
        L10nKey::DiffChangedFiles => "{count} changed files",
        L10nKey::DiffUntrackedCount => " · {count} untracked",
        L10nKey::DiffMoreFiles => {
            "… and {count} more changed files — run git diff in the terminal to see them."
        }
        L10nKey::DiffOversizedNotice => {
            "This working tree is too large to render ({summary}). Every file is collapsed — expand them one at a time, or run git diff in the terminal."
        }
        L10nKey::DiffTruncatedPerFile => {
            "Diff truncated at {limit} lines — run git diff in the terminal for the rest."
        }
        L10nKey::DiffTruncatedBudget => {
            "Body not loaded — past lyrel's diff budget. Run git diff in the terminal for this file."
        }
        L10nKey::DiffUntrackedHeader => "Untracked files ({count})",
        L10nKey::DiffMoreUntracked => {
            "… and {count} more — run git status in the terminal to see them."
        }
        L10nKey::DiffLines => "{count} diff lines",
        L10nKey::DiffChangedLines => {
            "{total} changed lines, {loaded} diff rows loaded before {cap} cut the rest"
        }
        L10nKey::DiffBudgetAndCap => "lyrel's budget and the per-file cap",
        L10nKey::DiffBudget => "lyrel's budget",
        L10nKey::DiffPerFileCap => "the per-file cap",
        L10nKey::DiffUntrackedSummary => "{count} untracked",
        L10nKey::DiffViewSplit => "Side by Side",
        L10nKey::DiffViewUnified => "Unified",
        L10nKey::DiffCopySelection => "Copy Selected Lines",
        L10nKey::PendingConnecting => "Connecting to {machine}…",
        L10nKey::PendingUnreachable => "Could not reach {machine}",
        L10nKey::WorktreePromptNeedsName => "The worktree needs a name",
        L10nKey::WorktreePromptTitle => "New Worktree Tab",
        L10nKey::WorktreePromptName => "Worktree Name",
        L10nKey::WorktreePromptBranch => "New Branch",
        L10nKey::WorktreePromptBase => "Start From",
        L10nKey::WorktreePromptCreating => "Creating…",
        L10nKey::WorktreePromptCreate => "Create",
        L10nKey::AppNewWorktreeFailed => "New worktree failed: {error}",
        L10nKey::HomeTimeJustNow => "just now",
        L10nKey::HomeTimeMinutesAgo => "{count} min ago",
        L10nKey::HomeTimeHourAgo => "1 hour ago",
        L10nKey::HomeTimeHoursAgo => "{count} hours ago",
        L10nKey::HomeTimeYesterday => "yesterday",
        L10nKey::HomeTimeDaysAgo => "{count} days ago",
        L10nKey::HomeTimeWeeksAgo => "{count} weeks ago",
        L10nKey::HomeTimeMonthsAgo => "{count} months ago",
        L10nKey::HomeTimeOverYearAgo => "over a year ago",
        L10nKey::HomeReopenNamed => "Reopen \"{name}\"",
        L10nKey::RemoteStripDisconnected => "Not connected to {machine}",
        L10nKey::RemoteStripConnecting => "Connecting to {machine}…",
        L10nKey::RemoteStripReconnecting => "Reconnecting to {machine}…",
        L10nKey::RemoteStripReconnectingAttempt => "Reconnecting to {machine}… (attempt {count})",
        L10nKey::RemoteStripReconnectingWhy => "Reconnecting to {machine}… — last failure: {error}",
        L10nKey::RemoteStripReconnectingAttemptWhy => {
            "Reconnecting to {machine}… (attempt {count}) — last failure: {error}"
        }
        L10nKey::RemoteStripPreempted => "This workspace was opened on {by}",
        L10nKey::RemoteStripFailed => "Not connected to {machine} — {error}",
        L10nKey::RemoteStripRouteLost => {
            "The connection profile for {machine} no longer exists — it cannot reconnect"
        }
        L10nKey::RemoteRouteParkedHint => {
            "Its connection profile is gone, so it will not reconnect on its own. The remote session is still there — connect with a new profile and it reappears in the workspace list."
        }
        L10nKey::RemoteNoticePreempted => "Opened elsewhere — typing has no effect",
        L10nKey::RemoteNoticeDisconnected => "Not connected — typing has no effect",
        L10nKey::RemoteActionRetryNow => "Retry Now",
        L10nKey::RemoteActionTakeBack => "Take Back",
        L10nKey::RemoteActionConnect => "Connect",
        L10nKey::RemoteActionRetry => "Retry",
        L10nKey::RemoteActionRemoveEntry => "Remove entry",
        L10nKey::RemoteNoConnectionDetails => {
            "This window is a workspace on {machine}, but lyrel has no connection details for it — check its SSH profile or ~/.ssh/config entry still exists."
        }
        L10nKey::RemoteThisComputer => "this computer",
        L10nKey::HostMetricsWaiting => "Waiting for host sample",
        L10nKey::HostMetricsConnecting => "Connecting to host",
        L10nKey::HostMetricsReconnecting => "Reconnecting to host",
        L10nKey::HostMetricsUnavailable => "Host metrics unavailable",
        L10nKey::HostMetricsDisconnected => "Host disconnected",
        L10nKey::HostMetricsFailed => "Host sampling failed: {reason}",
        L10nKey::HostMetricsError => "Host sampling failed",
        L10nKey::HostMetricsSample => "{host} · Whole-host CPU · RAM {used} / {total} GiB",
        L10nKey::RemoteProfileGone => "deleted profile",
        L10nKey::RemoteRestartTitle => "Restart lyrel's server on \"{machine}\"?",
        L10nKey::RemoteRestartBody => {
            "This ends every shell on {machine}, including ones this window is not showing. Workspaces and layouts are kept and come back with fresh shells."
        }
        L10nKey::RemoteReplaceBody => {
            "lyrel will install a matching server on {machine} and start it.\n\
             \n\
             Every session running on {machine} ends, including any this window is not \
             connected to."
        }
        L10nKey::RemoteRestartFailedTitle => "lyrel's server on \"{machine}\" was not restarted",
        L10nKey::RemoteRestartFailedBody => {
            "{error}\n\
             \n\
             Sessions still running there are on the older build. If they are \
             gone, reconnecting starts this build's server."
        }
        L10nKey::RemoteHostUnreachable => "could not reach {machine}: {error}",
        L10nKey::RemoteInstallTitle => "Install lyrel's server on \"{machine}\"?",
        L10nKey::RemoteInstallDetail => {
            "lyrel will write its server binary to {machine} so this machine can host \
             workspaces there. Nothing else on {machine} is touched, and no sudo is used.\n\
             \n\
             {path_label}\u{2003}{path}\n\
             {version_label}\u{2003}{version}\n\
             {size_label}\u{2003}{size}\n\
             {from_label}\u{2003}{from}\n\
             {sha_label}\u{2003}{sha256}\n\
             \n\
             {silent_upgrades}"
        }
        L10nKey::RemoteInstallPathLabel => "Path",
        L10nKey::RemoteInstallVersionLabel => "Version",
        L10nKey::RemoteInstallSizeLabel => "Size",
        L10nKey::RemoteInstallFromLabel => "From",
        L10nKey::RemoteInstallShaLabel => "SHA-256",
        L10nKey::RemoteInstallSilentUpgrades => "Later upgrades on this machine install silently.",
        L10nKey::RemoteInstallBytes => "bytes",
        L10nKey::RemoteMismatchTitle => "Update lyrel's server on \"{machine}\"?",
        L10nKey::RemoteMismatchDetail => {
            "{machine} runs server {running}, which this client ({wanted}) cannot speak. A matching server is installed there, but your sessions are on the one already running.\n\n{replace_server}\u{2003}replaces it with {wanted} and ends every session it is hosting.\n{cancel}\u{2003}leaves {machine} exactly as it is. This window will not connect."
        }
        L10nKey::RemoteMismatchReplaceServer => "Update Server",
        // Same button, opposite direction: the machine is ahead of this build,
        // so putting our server there takes it back a version. Calling that an
        // update would be a lie, and it is the kind that ends other people's
        // sessions on the way through.
        L10nKey::RemoteMismatchDowngradeServer => "Replace Server",
        L10nKey::RemoteMismatchUnknownBuild => "an unknown build",
        L10nKey::RemoteMismatchUnknownBuildFromExe => "an unknown build (from {exe})",
        L10nKey::RemoteServerOutdated => {
            "{machine} is running an old lyrel server ({build}) that this copy of lyrel \
             cannot talk to. Update it to connect."
        }
        L10nKey::RemoteServerTooNew => {
            "{machine} is running a newer lyrel server ({build}) than this copy of lyrel. \
             Update lyrel on this computer, or replace the server there with a matching one."
        }
        L10nKey::RemoteDaemonStartFailed => "lyrel's local server could not be started: {error}",
        L10nKey::RemoteDaemonUnreachable => "could not reach lyrel's local server: {error}",
        L10nKey::RemoteDaemonTooOld => {
            "this machine's daemon is an older build and cannot restart the server on {machine}. Quit lyrel (that stops the daemon), open it again, and retry."
        }
        L10nKey::RemoteProfileMissing => "that saved SSH profile no longer exists",
        L10nKey::RemoteAliasMissing => "\"{alias}\" is no longer in ~/.ssh/config",
        L10nKey::RemoteWslNoSsh => "a WSL workspace has no SSH connection",
        L10nKey::RemoteLocalStdioNoSsh => "a local --stdio workspace has no SSH connection",
        L10nKey::RemoteHostNotLyrel => "{machine} answered, but not as a lyrel server: {error}",
        L10nKey::RemoteWorkspaceListFailed => {
            "connected to {machine}, but its workspace list failed: {error}"
        }
        L10nKey::RemoteServerRestartFailed => {
            "could not restart lyrel's server on {machine}: {error}"
        }
        L10nKey::RemoteNoRouteToHost => "lyrel no longer has a way to reach {machine}",
        L10nKey::RemoteMachineTreeUnexpectedReply => {
            "the server answered a machine tree with {reply}"
        }
        L10nKey::RemoteMismatchVersionFromExe => "{version} (from {exe})",
        L10nKey::AppNoRunningCodingAgent => {
            "No running coding agent found — start one (claude, codex, …) in a pane first."
        }
        L10nKey::SwitcherThisComputer => "This Computer",
        L10nKey::SwitcherStartingServer => "Starting lyrel's server…",
        L10nKey::SwitcherDownloadingServerWithTotal => {
            "Downloading lyrel's server… {done} / {total}"
        }
        L10nKey::SwitcherDownloadingServerNoTotal => "Downloading lyrel's server… {done}",
        L10nKey::SwitcherCopyingServer => "Copying lyrel's server… {done} / {total}",
        L10nKey::SwitcherThisWindow => "This window",
        L10nKey::SwitcherOpen => "Open",
        L10nKey::SwitcherOffline => "Offline",
        L10nKey::SwitcherDisconnect => "Disconnect",
        L10nKey::SwitcherEditHost => "Edit Host…",
        L10nKey::SwitcherSaveAsHost => "Save as SSH Host…",
        L10nKey::SshSaveDroppedJumpHost => {
            "The jump host was left out — a saved host reaches its jump through another saved host."
        }
        L10nKey::SwitcherOpenInNewWindow => "Open in New Window",
        L10nKey::SwitcherRename => "Rename…",
        L10nKey::SwitcherPickAWorkspace => "Pick a workspace to see its tabs.",
        L10nKey::SwitcherNoTabs => "No tabs in this workspace.",
        L10nKey::SwitcherNoTabMatch => "No tab matches.",
        L10nKey::SwitcherTabsAfterOpening => "Open this workspace to see its tabs.",
        L10nKey::SwitcherOpenToManage => "Open this workspace to rename or stop it.",
        L10nKey::SwitcherConnectToUse => "Connect to this machine to open a workspace on it.",
        L10nKey::SwitcherOrphanPanes => {
            "Background panes — shells still running outside any window:"
        }
        L10nKey::SwitcherTabCount => "{n} tabs",
        L10nKey::SwitcherTabCountOne => "1 tab",
        L10nKey::SwitcherActiveTab => "Current",
        L10nKey::SwitcherHoldToSwitch => "Tab to move · release to switch",
        L10nKey::SwitcherTabToCrossColumns => "Tab to cross columns",
        L10nKey::SwitcherHintNavigate => "Navigate",
        L10nKey::SwitcherHintOpen => "Open",
        L10nKey::SearchHintNextScope => "Next scope",
        L10nKey::PanelFilesNameMatches => "File names",
        L10nKey::PanelSearchInContents => "In file contents",
        L10nKey::SwitcherHintNewWindow => "New window",
        L10nKey::SwitcherLocalHost => "local",
        L10nKey::SwitcherConnectingTo => "Connecting to {machine}…",
        L10nKey::SwitcherFormName => "Name",
        L10nKey::SwitcherFormHost => "Host",
        L10nKey::SwitcherFormNamePlaceholder => "Workspace name",
        L10nKey::SwitcherFormBack => "Back",
        L10nKey::SwitcherFormCreate => "Create",
        L10nKey::SwitcherFormPickHint => "↑↓ to choose · Enter to select · Esc to close",
        L10nKey::SshPromptPasswordFor => "Password for {user}@{host}",
        L10nKey::SshPromptPassphraseFor => "Passphrase for {key_path}",
        L10nKey::SshPromptTwoFactor => "Two-factor authentication",
        L10nKey::SshPromptUnknownHost => "Unknown host {host}",
        L10nKey::SshPromptHostKeyChanged => "Host key CHANGED — possible man-in-the-middle",
        L10nKey::SshPromptHostKeyChangedBody => {
            "The host key differs from the one previously trusted. This may be an attack."
        }
        L10nKey::SshPromptConnect => "Connect",
        L10nKey::SshPromptUnlock => "Unlock",
        L10nKey::SshPromptSubmit => "Submit",
        L10nKey::GitOpFailed => "git {op} failed",
        L10nKey::IoDenied => "You do not have permission.",
        L10nKey::IoGone => "It is not there any more.",
        L10nKey::IoNoSpace => "There is no space left on the disk.",
        L10nKey::IoReadOnly => "That location is read-only.",
        L10nKey::IoBusy => "Something else has it open.",
        L10nKey::IoTimedOut => "The machine did not answer in time.",
        L10nKey::TreeWindowOpenedEmpty => {
            "The server never handed over this window's tabs, so it opened empty. Nothing was lost — they come back when it answers. If it doesn't, run \"Restart lyrel server\" from the command palette."
        }
        L10nKey::CmdGroupTabsPanes => "Tabs & Panes",
        L10nKey::CmdGroupWorkspaces => "Workspaces",
        L10nKey::CmdGroupView => "View",
        L10nKey::CmdGroupGit => "Git",
        L10nKey::CmdGroupTerminal => "Terminal",
        L10nKey::CmdGroupSsh => "SSH",
        L10nKey::CmdGroupAgents => "Agents",
        L10nKey::CmdGroupApplication => "Application",
        L10nKey::CmdNewTab => "New Tab",
        L10nKey::CmdNewWindow => "New Window",
        L10nKey::CmdNewWorktreeTab => "New Worktree Tab…",
        L10nKey::CmdNewWorktreeTabSubtitle => "isolated checkout on a fresh branch",
        L10nKey::CmdNewGroup => "New Group",
        L10nKey::CmdNewGroupSubtitle => "an empty pinned group in the sidebar",
        L10nKey::CmdOpenFolderAsGroup => "Open Folder as Group…",
        L10nKey::CmdOpenFolderAsGroupSubtitle => "pin a folder; tabs opened in it join",
        L10nKey::CmdRenameTab => "Rename Tab…",
        L10nKey::CmdSplitRight => "Split Right",
        L10nKey::CmdSplitDown => "Split Down",
        L10nKey::CmdZoomPane => "Zoom Pane",
        L10nKey::CmdNextPane => "Next Pane",
        L10nKey::CmdPreviousPane => "Previous Pane",
        L10nKey::CmdFocusPaneLeft => "Focus Pane Left",
        L10nKey::CmdFocusPaneRight => "Focus Pane Right",
        L10nKey::CmdFocusPaneUp => "Focus Pane Up",
        L10nKey::CmdFocusPaneDown => "Focus Pane Down",
        L10nKey::CmdResizePaneLeft => "Resize Pane Left",
        L10nKey::CmdResizePaneRight => "Resize Pane Right",
        L10nKey::CmdResizePaneUp => "Resize Pane Up",
        L10nKey::CmdResizePaneDown => "Resize Pane Down",
        L10nKey::CmdSwapPaneNext => "Swap Pane Next",
        L10nKey::CmdSwapPanePrevious => "Swap Pane Previous",
        L10nKey::CmdNextTab => "Next Tab",
        L10nKey::CmdPreviousTab => "Previous Tab",
        L10nKey::CmdMoveTabLeft => "Move Tab Left",
        L10nKey::CmdMoveTabRight => "Move Tab Right",
        L10nKey::CmdRecentTabSwitcher => "Recent Tab Switcher",
        L10nKey::CmdRecentTabSwitcherReverse => "Recent Tab Switcher (Reverse)",
        L10nKey::CmdCopyWorkingDirectory => "Copy Working Directory",
        L10nKey::CmdCopySessionId => "Copy Session ID",
        L10nKey::CmdCopySessionIdSubtitle => "the coding agent's own session id",
        L10nKey::CmdNewAgentTab => "New Agent Tab",
        L10nKey::CmdNewAgentTabSubtitle => "open the coding agent you used last in a new tab",
        L10nKey::CmdForkSession => "Fork Session",
        L10nKey::CmdForkSessionSubtitle => "branch this agent session into a new tab",
        L10nKey::CmdMarkTabAsUnread => "Mark Tab as Unread",
        L10nKey::CmdHibernateTab => "Hibernate Tab",
        L10nKey::CmdHibernateTabSubtitle => {
            "stop its processes to free memory; selecting it wakes it"
        }
        L10nKey::CmdClosePaneTab => "Close Pane / Tab",
        L10nKey::CmdCloseWindow => "Close Window",
        L10nKey::CmdCloseWindowSubtitle => "shells keep running",
        L10nKey::CmdCloseOtherTabs => "Close Other Tabs",
        L10nKey::CmdCloseTabsToTheRight => "Close Tabs to the Right",
        L10nKey::CmdReopenClosedTab => "Reopen Closed Tab",
        L10nKey::CmdNewWorkspace => "New Workspace…",
        L10nKey::CmdSwitchWorkspace => "Switch Workspace…",
        L10nKey::CmdRenameWorkspace => "Rename Workspace…",
        L10nKey::CmdStopWorkspace => "Stop Workspace…",
        L10nKey::CmdStopWorkspaceSubtitle => "ends its shells, keeps the layout",
        L10nKey::CmdDeleteWorkspace => "Delete Workspace…",
        L10nKey::CmdDeleteWorkspaceSubtitle => "ends its shells and forgets the layout",
        L10nKey::CmdShowLeftSidebar => "Show Left Sidebar",
        L10nKey::CmdHideLeftSidebar => "Hide Left Sidebar",
        L10nKey::CmdHideRightPanel => "Hide Right Panel",
        L10nKey::CmdShowRightPanel => "Show Right Panel",
        L10nKey::CmdShowCodePanel => "Show Code Panel",
        L10nKey::CmdRightPanelInfo => "Right Panel: Info",
        L10nKey::CmdRightPanelChanges => "Right Panel: Changes",
        L10nKey::CmdRightPanelFiles => "Right Panel: Files",
        L10nKey::CmdRightPanelSearch => "Right Panel: Search",
        L10nKey::CmdRightPanelGitHub => "Right Panel: GitHub",
        L10nKey::CmdChangeTheme => "Change Theme…",
        L10nKey::CmdResetFontSize => "Reset Font Size",
        L10nKey::CmdEnterFullScreen => "Enter Full Screen",
        L10nKey::CmdToggleDiffViewMode => "Toggle Unified / Side-by-Side Diff",
        L10nKey::CmdDocumentDock => "Document: Dock Beside Terminal",
        L10nKey::CmdDocumentFill => "Document: Fill Window",
        L10nKey::CmdToggleDocumentFill => "Toggle Document Fill / Dock",
        L10nKey::CmdDocumentWidthThird => "Document: Third Width",
        L10nKey::CmdDocumentWidthHalf => "Document: Half Width",
        L10nKey::CmdDocumentWidthTwoThirds => "Document: Two-Thirds Width",
        L10nKey::CmdToggleDocumentPreview => "Document: Toggle Markdown Live Editing / Source",
        L10nKey::CmdNewMarkdownDocument => "New Markdown Document",
        L10nKey::CmdToggleDocumentWrap => "Document: Toggle Word Wrap",
        L10nKey::CmdEditorTransformUppercase => "Document: Transform to Uppercase",
        L10nKey::CmdEditorTransformLowercase => "Document: Transform to Lowercase",
        L10nKey::CmdEditorTransformTitleCase => "Document: Transform to Title Case",
        L10nKey::CmdEditorTrimTrailingWhitespace => "Document: Trim Trailing Whitespace",
        L10nKey::CmdEditorJoinLines => "Document: Join Lines",
        L10nKey::CmdEditorRemoveSurroundingBrackets => "Document: Remove Surrounding Brackets",
        L10nKey::CmdGitCommit => "Git: Commit",
        L10nKey::CmdGitStageAll => "Git: Stage All Changes",
        L10nKey::CmdGitUnstageAll => "Git: Unstage All Changes",
        L10nKey::CmdGitDiscardAll => "Git: Discard All Changes",
        L10nKey::CmdGitDiscardAllSubtitle => {
            "Throws away every uncommitted change in the working tree."
        }
        L10nKey::CmdGitCheckoutTo => "Git: Checkout to…",
        L10nKey::CmdGitCreateBranch => "Git: Create Branch…",
        L10nKey::CmdGitSync => "Git: Sync",
        L10nKey::CmdGitSyncSubtitle => "Pull, then push.",
        L10nKey::CmdGitPush => "Git: Push",
        L10nKey::CmdGitPull => "Git: Pull",
        L10nKey::CmdGitFetch => "Git: Fetch",
        L10nKey::CmdGitToggleGraph => "Git: Toggle Commit History",
        L10nKey::CmdClearScrollback => "Clear Scrollback",
        L10nKey::CmdFindInTerminal => "Find in Terminal…",
        L10nKey::CmdToggleComposer => "Toggle Message Composer",
        L10nKey::ComposerPlaceholder => "Message {agent}   ·   @ files   / commands",
        L10nKey::ComposerPlaceholderFiles => "Message {agent}   ·   @ files",
        L10nKey::ComposerSendTip => "Send (Enter)  ·  Shift+Enter for a new line",
        L10nKey::ComposerAttach => "Attach files or images",
        L10nKey::ComposerMenuCommands => "Commands",
        L10nKey::ComposerMenuFiles => "Files",
        L10nKey::ComposerAgentAsking => {
            "{agent} is asking something. Answer it in the terminal, then send."
        }
        L10nKey::CmdFindNext => "Find Next",
        L10nKey::CmdFindPrevious => "Find Previous",
        L10nKey::CmdCopy => "Copy",
        L10nKey::CmdCut => "Cut",
        L10nKey::CmdPaste => "Paste",
        L10nKey::CmdAlternatePaste => "Paste (outside full-screen apps)",
        L10nKey::CmdSelectAll => "Select All",
        L10nKey::CmdSshAddConnection => "SSH: Add Connection…",
        L10nKey::CmdSshManageProfiles => "SSH: Manage Profiles…",
        L10nKey::CmdSshReconnect => "SSH: Reconnect",
        L10nKey::CmdSshRemoteFiles => "SSH: Remote Files",
        L10nKey::CmdSshPortForwarding => "SSH: Port Forwarding",
        L10nKey::CmdSshSaveConnection => "SSH: Save Connection as Host…",
        L10nKey::CmdSshSaveConnectionSubtitle => "Keep this connection as a saved host.",
        L10nKey::CmdSshConnectWithInput => "SSH: Connect {input}",
        L10nKey::CmdAgentSendSelection => "Agent: Send Selection",
        L10nKey::CmdAgentSendSelectionSubtitle => "selection → running coding agent",
        L10nKey::CmdAgentSendGitDiffForReview => "Agent: Send Git Diff for Review",
        L10nKey::CmdAgentSendGitDiffSubtitle => "git diff → running coding agent",
        L10nKey::CmdSettings => "Settings…",
        L10nKey::CmdKeyboardShortcuts => "Keyboard Shortcuts",
        L10nKey::CmdAboutLyrel => "About lyrel",
        L10nKey::CmdCheckForUpdates => "Check for Updates…",
        L10nKey::CmdDocumentation => "Documentation",
        L10nKey::CmdProjectPage => "Project page",
        L10nKey::CmdReportIssue => "Report an Issue…",
        L10nKey::CmdRestartServer => "Restart lyrel server…",
        L10nKey::CmdRestartServerSubtitle => "ends every running shell; layout is kept",
        L10nKey::CmdQuitLyrel => "Quit lyrel",
        L10nKey::CmdQuitLyrelSubtitle => "stops the server; every running shell ends",
        L10nKey::CmdQuickConnect => "Connect to \"{target}\"",
        L10nKey::CmdQuickConnectSaveProfile => "Save \"{target}\" as profile…",
        L10nKey::CmdRecent => "Recent",
        L10nKey::AppRestartServerTitle => "Restart lyrel server?",
        L10nKey::AppRestartServerFailed => "Could not restart lyrel server: {error}",
        L10nKey::AppRestartServerMismatchDetail => {
            "lyrel server holding your shells speaks protocol {protocol} (build v{build}); this app speaks {ours}, so your tabs are out of reach.\n\nQuit: nothing changes — lyrel server and your shells keep running.\nRestart: tabs come back with fresh shells; anything running now is killed."
        }
        L10nKey::AppRestartServerDialectDetail => {
            "lyrel server holding your shells speaks control dialect v{dialect} (build v{build}); this app speaks v{ours}, so every window opens empty.\n\nQuit: nothing changes — lyrel server and your shells keep running.\nRestart: tabs come back with fresh shells; anything running now is killed."
        }
        L10nKey::AppRestartServerDialectNewerDetail => {
            "lyrel server holding your shells speaks control dialect v{dialect} (build v{build}); this app speaks v{ours}, so every window opens empty.\n\nQuit and install the newer build: the real fix — your shells survive it.\nRestart: tabs come back with fresh shells; anything running now is killed."
        }
        L10nKey::AppRestartServerOldDetail => {
            "lyrel server holding your shells predates the version handshake, so this app can't tell what it speaks.\n\nQuit: nothing changes — lyrel server and your shells keep running.\nRestart: tabs come back with fresh shells; anything running now is killed."
        }
        L10nKey::AppRestart => "Restart",
        L10nKey::AppRestartServerNoServer => {
            "{label} has no server of its own — it is a program this computer runs over --stdio. Stop its workspace instead."
        }
        L10nKey::AppRestartServerBody => {
            "This ends every shell on this computer. Your tabs and layout are kept and reopen with fresh shells."
        }
        L10nKey::ConfigQuarantinedStartup => {
            "config.json could not be parsed. lyrel is on default settings and kept the file's contents beside it as config.json.corrupt. Fix it and lyrel reloads; until then, Settings changes are not saved."
        }
        L10nKey::ConfigQuarantinedReload => {
            "The edited config.json could not be parsed. lyrel kept the settings it is running on and set the file's contents aside as config.json.corrupt. Fix it and lyrel reloads; saving a setting first overwrites it."
        }
        L10nKey::ConfigUnreadableStartup => {
            "config.json could not be read. lyrel is on default settings and left the file exactly as it is. Fix its permissions or contents and lyrel reloads; until then, Settings changes are not saved."
        }
        L10nKey::ConfigUnreadableReload => {
            "config.json could not be read. lyrel kept the settings it is running on and left the file exactly as it is. Fix its permissions or contents and lyrel reloads; saving a setting first overwrites it."
        }
        L10nKey::AppWorktreeRemoveDetailDirty => {
            "The closed tab's worktree at {path} has uncommitted changes. They are saved to refs/lyrel/trash/{name} before anything is deleted."
        }
        L10nKey::AppWorktreeRemoveDetailClean => "The closed tab's worktree at {path} is clean.",
        L10nKey::AppWorktreeRemoveTitle => "Remove worktree \"{branch}\"?",
        L10nKey::AppWorktreeDiscardAndRemove => "Discard Changes & Remove",
        L10nKey::AppWorktreeRemove => "Remove Worktree",
        L10nKey::AppWorktreeKeep => "Keep",
        L10nKey::AppReopenTabFailed => "Could not reopen the tab: no terminal started",
        L10nKey::AppOpenTerminalFailed => "Could not open a terminal: {error}",
        L10nKey::AppTabsNotRestored => "{count} tabs from last time could not be reopened",
        L10nKey::AppFullscreenEntered => "Fullscreen — press {key} to leave",
        L10nKey::AppFullscreenEnteredNoKey => {
            "Fullscreen — the window buttons are hidden until you leave"
        }
        L10nKey::LaunchWorkspacesLeftRunning => {
            "Only this window was restored — {count} workspaces are still running in the background. Reopen them from the sidebar."
        }
        L10nKey::AppSshConnectionFailed => "SSH connection failed: {error}",
        L10nKey::AppSshReconnectFailed => "SSH reconnect failed: {error}",
        L10nKey::AppSplitPaneFailed => "Could not split the pane: {error}",
        L10nKey::PaneDragHandleTooltip => "Drag to move or park; click for pane commands",
        L10nKey::PaneMenuTitle => "Pane commands",
        L10nKey::PaneMoveToTab => "Move pane to new tab",
        L10nKey::PaneEnterPip => "Move pane to picture-in-picture",
        L10nKey::PaneRestorePip => "Restore picture-in-picture to tiled layout",
        L10nKey::PaneTogglePip => "Toggle pane picture-in-picture",
        L10nKey::PanePipTitle => "Picture-in-picture terminal",
        L10nKey::PanePark => "Park pane",
        L10nKey::PaneRestoreParked => "Restore parked pane…",
        L10nKey::ParkedDeckTitle => "Parked panes",
        L10nKey::ParkedRestore => "Restore",
        L10nKey::ParkedAlive => "Running",
        L10nKey::ParkedExited => "Exited",
        L10nKey::ParkedEmpty => "All panes are parked. Restore a card or create a pane.",
        L10nKey::ParkDropTarget => "Park",
        L10nKey::ParkedMore => "More…",
        L10nKey::ParkedOverflowHint => {
            "↑↓ / Home / End: select · Enter: preview, then restore · Esc: back"
        }
        L10nKey::PanePipMove => {
            "Move picture-in-picture; drag to the bottom target to park (arrow keys; Escape cancels)"
        }
        L10nKey::PanePipResize => {
            "Resize picture-in-picture (arrow keys; Escape returns to terminal)"
        }
        L10nKey::PaneFindScrollback => "Find in scrollback…",
        L10nKey::PaneOpenScrollback => "Open scrollback in editor",
        L10nKey::PaneClearScreen => "Clear screen",
        L10nKey::PaneClearAll => "Clear screen and scrollback",
        L10nKey::PaneRestartShell => "Restart shell…",
        L10nKey::PaneRestartShellDetail => {
            "This stops the program running in this pane and starts a new shell on the same machine. Other panes are not restarted."
        }
        L10nKey::PaneRestartFailed => "Could not restart the shell: {error}",
        L10nKey::PaneClose => "Close pane",
        L10nKey::AppWorktreeRemoved => "Removed worktree \"{branch}\"",
        L10nKey::AppWorktreeRemoveFailed => "Worktree removal failed: {error}",
        L10nKey::WorktreePromptAgent => "Start",
        L10nKey::WorktreePromptShell => "Shell",
        L10nKey::WorktreePromptTask => "Task",
        L10nKey::WorktreePromptSetup => "Runs .lyrel/setup first",
        L10nKey::WorktreePromptSetupHint => {
            "No .lyrel/setup. Add one to run `{command}` in new worktrees"
        }
        L10nKey::AppWorktreeSetupTitle => "Run this repo's setup script?",
        L10nKey::AppWorktreeSetupDetail => {
            "{path} runs in the new tab before anything else. Approve it only if you trust this repo; lyrel asks again whenever the script changes."
        }
        L10nKey::AppWorktreeSetupRun => "Run Setup",
        L10nKey::AppWorktreeSetupSkip => "Skip",
        L10nKey::AppWorktreeNotCarried => "Not copied from .worktreeinclude: {paths}",
        L10nKey::AppWorktreeRemovedBranchKept => {
            "Removed worktree; kept branch \"{branch}\", which has unmerged commits"
        }
        L10nKey::AppPaneNoCodingAgent => "This pane isn't running a coding agent",
        L10nKey::AppForkNoCommand => "lyrel has no fork command for {name}",
        L10nKey::AppForkLocalOnly => {
            "{name} sessions can't be forked from inside an SSH or WSL session"
        }
        L10nKey::AppForkNoSessionId => {
            "lyrel hasn't seen a {name} session id in this pane — install its hooks in Settings → Integrations"
        }
        L10nKey::AppForkSessionIdNotToken => "{name}'s session id isn't a plain token",
        L10nKey::AppForkMidTurn => "{name} is mid-turn — the fork won't include the turn in flight",
        L10nKey::AppTabNoWorkingDirectory => "This tab has no working directory yet",
        L10nKey::AppNothingSelected => "Nothing selected — select some terminal output first.",
        L10nKey::AppPaneNoKnownDirectory => "This pane has no known directory.",
        L10nKey::AppNoUncommittedChanges => {
            "No uncommitted changes in {cwd} (or not a git repository)."
        }
        L10nKey::AppCmdAgentLaunchTitle => "Agent: {name}",
        L10nKey::AppNoAgentOnPath => "No coding agent was found on this machine's PATH",
        L10nKey::AppNoAgentSeenHere => {
            "No coding agent has run in this workspace yet — start one by hand once and it will be offered here"
        }
        L10nKey::AppAgentLaunchSaved => "{name} will now launch as: {command}",
        L10nKey::AppAgentLaunchArgsUnknown => {
            "{name} did not report the arguments it was started with"
        }
        L10nKey::AppCmdShellTitle => "Shell: {title}",
        L10nKey::AppPlaceholderDescription => "description",
        L10nKey::AppPlaceholderSshQuickConnect => "user@host  or  user@host:port",
        L10nKey::AppPlaceholderLoginShell => "login shell",
        L10nKey::AppPlaceholderNone => "none",
        L10nKey::AppPlaceholderOpenInDefaultApp => "open in default app",
        L10nKey::AppThemeColorBackground => "Background",
        L10nKey::AppThemeColorForeground => "Foreground",
        L10nKey::AppThemeColorAccent => "Accent",
        L10nKey::AppThemeColorCursor => "Cursor",
        L10nKey::AppThemeColorSelection => "Selection",
        L10nKey::AppThemeAnsiBlack => "Black",
        L10nKey::AppThemeAnsiRed => "Red",
        L10nKey::AppThemeAnsiGreen => "Green",
        L10nKey::AppThemeAnsiYellow => "Yellow",
        L10nKey::AppThemeAnsiBlue => "Blue",
        L10nKey::AppThemeAnsiMagenta => "Magenta",
        L10nKey::AppThemeAnsiCyan => "Cyan",
        L10nKey::AppThemeAnsiWhite => "White",
        L10nKey::AppThemeAnsiBrightBlack => "Bright black",
        L10nKey::AppThemeAnsiBrightRed => "Bright red",
        L10nKey::AppThemeAnsiBrightGreen => "Bright green",
        L10nKey::AppThemeAnsiBrightYellow => "Bright yellow",
        L10nKey::AppThemeAnsiBrightBlue => "Bright blue",
        L10nKey::AppThemeAnsiBrightMagenta => "Bright magenta",
        L10nKey::AppThemeAnsiBrightCyan => "Bright cyan",
        L10nKey::AppThemeAnsiBrightWhite => "Bright white",
        L10nKey::AppAgentHooksThisComputer => "This Computer",
        L10nKey::AppAgentHooksRemoteMachine => "Remote machine",
        L10nKey::AppAgentHooksNoHomeDir => {
            "lyrel could not work out this computer's home directory, so there is nowhere to install to."
        }
        L10nKey::AppAgentHooksOffline => {
            "Not connected to this machine, so its agent config can't be read or written. Open a workspace on it and come back."
        }
        L10nKey::AppAgentHooksHomeDirUnresolved => "cannot resolve home directory",
        L10nKey::AppAgentHooksInstalled => "Installed",
        L10nKey::AppAgentHooksRemoved => "Removed",
        L10nKey::AppAgentHooksNothingInstalled => "Nothing installed; nothing to remove",
        L10nKey::AppAgentHooksNoLyrelHooks => "No lyrel hooks found; nothing to remove",
        L10nKey::AppAgentHooksInstallFailed => "Could not install hooks: {error}",
        L10nKey::AppAgentHooksRemoveFailed => "Could not remove hooks: {error}",
        L10nKey::AppKeybindingDisplacedNote => {
            "{action} took the shortcut from {previous}, which is now unset."
        }
        L10nKey::AppLocalServerName => "the local server",
        L10nKey::AppSshParseUnbalancedQuotes => "Unbalanced quotes in the SSH command",
        L10nKey::AppSshParseNoRemoteCommands => "Remote commands aren't supported here",
        L10nKey::AppSshParseFlagNeedsValue => "-{flag} needs a value",
        L10nKey::AppSshParseInvalidPort => "Invalid port \"{value}\"",
        L10nKey::AppSshParseUnsupportedOption => "Unsupported option \"{option}\"",
        L10nKey::AppSshParseEnterHost => "Enter a host to connect to",
        L10nKey::AppSshParseBadHost => "Invalid host \"{host}\"",
        L10nKey::AppMenuMinimize => "Minimize",
        L10nKey::AppMenuZoom => "Zoom",
        L10nKey::SwitcherStatusRestarting => "restarting…",
        L10nKey::SwitcherStatusInstalling => "installing…",
        L10nKey::SwitcherStatusConnecting => "connecting…",
        L10nKey::SwitcherStatusConnectFailed => "couldn't connect",
        L10nKey::SwitcherStatusNotConnected => "not connected",
        L10nKey::SwitcherStatusReconnecting => "reconnecting…",
        L10nKey::SwitcherStatusTakenOver => "taken over",
        L10nKey::SettingsFontDefault => "Default (match primary)",
        L10nKey::SettingsUiFontDefault => "Default (system UI font)",
        L10nKey::ForwardDescriptionPlaceholder => "what it's for",
        L10nKey::SettingsShellDefaultLoginShell => "your login shell",
        L10nKey::SettingsShellDetected => "Shells lyrel found",
        L10nKey::SftpErrorUnexpectedReply => "unexpected reply: {reply}",
        L10nKey::SftpErrorUnsafeRemoteName => "refusing unsafe remote name {name}",
        L10nKey::SftpErrorNoFreeLocalName => {
            "no free name left in Downloads for {name} — move or delete the older copies"
        }
        L10nKey::SftpReplaceTitle => "Replace what is already there?",
        L10nKey::SftpReplaceBody => {
            "{names} already exist in this folder. Uploading overwrites them."
        }
        L10nKey::Replace => "Replace",
        L10nKey::SftpErrorInvalidOctalMode => "invalid octal mode",
        L10nKey::SettingsDaemonStaleDescInPlace => {
            "lyrel was updated. The server can swap itself in place and keep your shells; only built-in SSH panes close."
        }
        L10nKey::AppRestartServerBodyInPlace => {
            "lyrel server swaps itself for this build in place: your shells keep running, and the window reconnects a moment later. Panes on lyrel's built-in SSH client are the exception — those close and need reopening."
        }
        L10nKey::PaneRestoredScreenBanner => {
            "restored screen — this shell is new, nothing above it is still running"
        }
        L10nKey::SettingsPerPaneHistory => "Separate command history per pane",
        L10nKey::SettingsPerPaneHistoryDescription => {
            "↑ walks this pane's own history. bash and zsh only."
        }
        L10nKey::IntegrationNoticeBlocked => {
            "\u{201c}{wrapper}\u{201d} is intercepting shell reports in this pane, so inline completion and the Ctrl+R menu are unavailable. The shell's own history search still works."
        }
        L10nKey::IntegrationNoticeNotEngaged => {
            "lyrel shell integration hasn't engaged in this pane, so inline completion and the Ctrl+R menu are unavailable. Usual causes: a shell you started with your own arguments, a PTY wrapper, or an unsupported shell."
        }
        L10nKey::PaneTitleDisconnected => "{title} — disconnected",
        L10nKey::PaneTitleProcessExited => "{title} — process exited",
        L10nKey::LoopbackForwardFailed => "Couldn't forward :{port} — {error}",
        L10nKey::TrayTooltipAgents => "lyrel — {parts}",
        L10nKey::TrayAgentSep => ", ",
        L10nKey::CursorShapeBlock => "Block",
        L10nKey::CursorShapeBar => "Bar",
        L10nKey::CursorShapeUnderline => "Underline",
        L10nKey::PromptCursorShapeFollow => "Follow",
        L10nKey::PaletteTryDifferentSearch => "Try a different search.",
        L10nKey::CompletionListingRemote => "listing remote…",
        L10nKey::CompletionRemoteListingFailed => "remote listing failed — {error}",
        L10nKey::CmdUpdateLocalServer => "Update lyrel server on this computer…",
        L10nKey::CmdUpdateLocalServerSubtitle => "restarts it onto this app's build",
        L10nKey::CmdUpdateRemoteServer => "Update lyrel server on \"{machine}\"…",
        L10nKey::CmdUpdateRemoteServerSubtitle => {
            "reinstalls this build's server there; ends every session on it"
        }
        L10nKey::AppLocalServerAlreadyCurrent => {
            "lyrel server on this computer is already running this build ({build})."
        }
        L10nKey::RemoteUpdateBody => {
            "lyrel will install this build's server on {machine} — even over one that already speaks the same version — and restart it.\n\nEvery session running on {machine} ends, including any this window is not connected to."
        }
        L10nKey::RemoteUpdateNeedsLocalServer => {
            "lyrel server on this computer is too old to update the one on {machine}. Update this computer's server first, then try again."
        }
        L10nKey::PanelMoreChangedFiles => {
            "… and {count} more changed files — run git diff to see them."
        }
        L10nKey::ScmFilesChanged => "{count} files changed",
        L10nKey::ScmStagedFileCount => "{count} files staged",
        L10nKey::AppMenuAbout => "About lyrel",
        L10nKey::AppMenuCheckForUpdates => "Check for Updates…",
        L10nKey::AppMenuSettings => "Settings…",
        L10nKey::AppMenuServices => "Services",
        L10nKey::AppMenuHideApp => "Hide lyrel",
        L10nKey::AppMenuHideOthers => "Hide Others",
        L10nKey::AppMenuShowAll => "Show All",
        L10nKey::AppMenuQuit => "Quit lyrel",
        L10nKey::AppMenuFile => "File",
        L10nKey::AppMenuEdit => "Edit",
        L10nKey::AppMenuView => "View",
        L10nKey::AppMenuWindow => "Window",
        L10nKey::AppMenuHelp => "Help",
        L10nKey::AppMenuNewTab => "New Tab",
        L10nKey::AppMenuNewWorkspace => "New Workspace…",
        L10nKey::AppMenuNewWorktreeTab => "New Worktree Tab…",
        L10nKey::AppMenuSplitRight => "Split Right",
        L10nKey::AppMenuSplitLeft => "Split Left",
        L10nKey::AppMenuSplitDown => "Split Down",
        L10nKey::AppMenuSplitUp => "Split Up",
        L10nKey::AppMenuRenameTab => "Rename Tab…",
        L10nKey::AppMenuCopyWorkingDirectory => "Copy Working Directory",
        L10nKey::AppMenuCopySessionId => "Copy Session ID",
        L10nKey::AppMenuForkSession => "Fork Session",
        L10nKey::AppMenuSaveAgentLaunchArgs => "Set Current Launch Args as Default",
        L10nKey::AppMenuClosePaneTab => "Close",
        L10nKey::AppMenuCloseOtherTabs => "Close Other Tabs",
        L10nKey::AppMenuCloseTabsRight => "Close Tabs to the Right",
        L10nKey::AppMenuReopenClosedTab => "Reopen Closed Tab",
        L10nKey::AppMenuRenameWorkspace => "Rename Workspace…",
        L10nKey::AppMenuStopWorkspace => "Stop Workspace…",
        L10nKey::AppMenuDeleteWorkspace => "Delete Workspace…",
        L10nKey::AppMenuUndo => "Undo",
        L10nKey::AppMenuRedo => "Redo",
        L10nKey::AppMenuCut => "Cut",
        L10nKey::AppMenuCopy => "Copy",
        L10nKey::AppMenuPaste => "Paste",
        L10nKey::AppMenuSelectAll => "Select All",
        L10nKey::AppMenuFind => "Find…",
        L10nKey::AppMenuFindNext => "Find Next",
        L10nKey::AppMenuFindPrevious => "Find Previous",
        L10nKey::AppMenuSearchEverywhere => "Search…",
        L10nKey::AppMenuIncreaseFontSize => "Increase Font Size",
        L10nKey::AppMenuDecreaseFontSize => "Decrease Font Size",
        L10nKey::AppMenuResetFontSize => "Reset Font Size",
        L10nKey::AppMenuLeftSidebar => "Left Sidebar",
        L10nKey::AppMenuRightPanel => "Right Panel",
        L10nKey::AppMenuCodePanel => "Code Panel",
        L10nKey::AppMenuFocusNextPane => "Focus Next Pane",
        L10nKey::AppMenuFocusPreviousPane => "Focus Previous Pane",
        L10nKey::AppMenuZoomPane => "Zoom Pane",
        L10nKey::AppMenuClearScrollback => "Clear Scrollback",
        L10nKey::AppMenuOpenLink => "Open",
        L10nKey::AppMenuOpenLinkWithDefaultApp => "Open with Default App",
        L10nKey::AppMenuRevealInFinder => "Show in Finder",
        L10nKey::AppMenuRevealInFolder => "Show Containing Folder",
        L10nKey::AppMenuCopyLinkPath => "Copy Path",
        L10nKey::AppMenuDocumentation => "lyrel Documentation",
        L10nKey::AppMenuKeyboardShortcuts => "Keyboard Shortcuts",
        L10nKey::AppMenuProjectPage => "Project page",
        L10nKey::AppMenuReportIssue => "Report an Issue…",
        L10nKey::AppMenuRestartServer => "Restart lyrel server…",
        L10nKey::WindowUntitled => "Untitled",
        L10nKey::TrayShowLyrel => "Show lyrel",
        L10nKey::TrayNotifications => "Notifications",
        L10nKey::TrayAgentNeedsInput => "needs input",
        L10nKey::AgentStatusWorking => "Working",
        L10nKey::AgentStatusWaiting => "Needs input",
        L10nKey::AgentStatusDone => "Done",
        L10nKey::NotifyCommandFinished => "Command finished after {secs}s",
        L10nKey::NotifyCommandFinishedWithCommand => "{command} — finished after {secs}s",
        L10nKey::NotifyAgentFinished => "Finished after {secs}s",
        L10nKey::NotifyAgentWaiting => "Waiting for your input",
        L10nKey::NotifyTurnFinished => "Turn finished",
        L10nKey::TabTooltipMore => "More",
        L10nKey::TabTooltipShowSidebar => "Show Sidebar",
        L10nKey::TabTooltipHideSidebar => "Hide Sidebar",
        L10nKey::TabTooltipHideDetailPanel => "Hide Detail Panel",
        L10nKey::TabTooltipShowDetailPanel => "Show Detail Panel",
        L10nKey::TabTooltipZoomed => "Pane zoomed — other panes hidden",
        L10nKey::TabMenuLocalShells => "Local",
        L10nKey::TabMenuAddHost => "Add SSH Host…",
        L10nKey::TabMenuAllHosts => "All SSH Hosts…",
        L10nKey::TabMenuOtherShells => "Other Shells…",
        L10nKey::TabMenuOtherAgents => "Other Agents…",
        L10nKey::TabMenuSplitHint => "Hold {key} to split",
        L10nKey::TabUnnamedShell => "Shell {n}",
        L10nKey::ShellDefault => "default",
        L10nKey::SidebarUngroupedGroup => "Ungrouped",
        L10nKey::SidebarMoveToGroup => "Move to Group",
        L10nKey::SidebarNewGroup => "New Group…",
        L10nKey::SidebarRemoveFromGroup => "Remove from Group",
        L10nKey::SidebarNewGroupName => "New Group",
        L10nKey::SidebarRenameGroup => "Rename Group",
        L10nKey::SidebarPinGroup => "Pin Group",
        L10nKey::SidebarUnpinGroup => "Unpin",
        L10nKey::SidebarGroupNewTab => "New Tab",
        L10nKey::SidebarSetGroupFolder => "Set Folder…",
        L10nKey::SidebarUseCurrentTabFolder => "Use Current Tab's Folder",
        L10nKey::SidebarClearGroupFolder => "Clear Folder",
        L10nKey::SidebarDeleteGroup => "Delete Group",
        L10nKey::SidebarDropToPin => "Drop here to pin",
        L10nKey::TabContextCloseTab => "Close Tab",
        L10nKey::TerminalContextClear => "Clear",
        L10nKey::TabContextCloseTabsBelow => "Close Tabs Below",
        L10nKey::AppAgentHooksOpFailed => "Failed: {error}",
        L10nKey::AppMenuEnterFullscreen => "Enter Full Screen",
        L10nKey::HomeTimeOverWeekAgo => "over a week ago",
        L10nKey::Search => "Search",
        L10nKey::SettingsDaemonStaleRestart => "Restart lyrel server",
        L10nKey::SettingsNoneLower => "none",
        L10nKey::SettingsSearchCommandLineToolTitle => "Command line tool",
        L10nKey::TabContextMarkUnread => "Mark as Unread",
        L10nKey::TabContextHibernate => "Hibernate",
        L10nKey::TabContextWake => "Wake",
        L10nKey::TabTooltipAsleep => "Hibernated — select to wake",
        L10nKey::TabWakeFailed => "Could not wake the tab: none of its panes could be started",
        // ---- The right panel's GitHub tab ----
        L10nKey::GitHubIssues => "Issues",
        L10nKey::GitHubPulls => "Pull Requests",
        L10nKey::GitHubOpen => "Open",
        L10nKey::GitHubClosed => "Closed",
        L10nKey::GitHubMerged => "Merged",
        L10nKey::GitHubDraft => "Draft",
        L10nKey::GitHubNotPlanned => "Not planned",
        L10nKey::GitHubRefresh => "Refresh",
        L10nKey::GitHubOpenOnGitHub => "Open on GitHub",
        L10nKey::GitHubShowRemote => "Show issues from",
        L10nKey::GitHubLoadMore => "Load more",
        L10nKey::GitHubNoRemote => "No GitHub remote",
        L10nKey::GitHubNoRemoteHint => "None of this repository's remotes point at github.com.",
        L10nKey::GitHubNoIssues => "No issues match.",
        L10nKey::GitHubNoPulls => "No pull requests match.",
        L10nKey::GitHubSignInHint => "Sign in with `gh auth login` in a terminal, then refresh.",
        L10nKey::GitHubNotFoundSignedOut => {
            "GitHub did not find this repository. If it is private, sign in first."
        }
        L10nKey::GitHubNotFoundSignedIn => {
            "GitHub did not find this repository, or none of your gh accounts can see it."
        }
        L10nKey::GitHubUnauthorized => "GitHub rejected the saved sign-in.",
        L10nKey::GitHubRateLimited => "GitHub's rate limit is used up.",
        L10nKey::GitHubRateLimitResetIn => "It resets in {n} min.",
        L10nKey::GitHubRateLimitSignedOut => {
            "Signed out, GitHub allows 60 requests an hour. Sign in with `gh auth login` for more."
        }
        L10nKey::GitHubForbidden => "GitHub refused the request.",
        L10nKey::GitHubNetworkError => "Could not reach GitHub.",
        L10nKey::GitHubHttpError => "GitHub answered with an error ({code}).",
        L10nKey::GitHubDecodeError => "GitHub sent a response lyrel could not read.",
        L10nKey::GitHubMoreOnGitHub => "More on GitHub",
        L10nKey::GitHubNoDescription => "No description provided.",
        L10nKey::GitHubFilterByLabel => "Show only this label",
        L10nKey::GitHubClearLabel => "Clear label filter",
        L10nKey::GitHubImage => "image",
        L10nKey::GitHubComments => "{count} comments",
        L10nKey::GitHubCommits => "{count} commits",
        L10nKey::GitHubOpenedAt => "opened {when}",
        L10nKey::GitHubUpdatedAt => "updated {when}",
        L10nKey::GitHubChecks => "Checks",
        L10nKey::GitHubChecksPassed => "{passed} of {total} passed",
        L10nKey::GitHubChecksNoneCounted => "No checks with a result",
        L10nKey::GitHubCheckPassed => "Passed",
        L10nKey::GitHubCheckFailed => "Failed",
        L10nKey::GitHubCheckPending => "In progress",
        L10nKey::GitHubCheckSkipped => "Skipped",
        L10nKey::GitHubReviews => "Reviews",
        L10nKey::GitHubReviewApproved => "Approved",
        L10nKey::GitHubReviewChangesRequested => "Changes requested",
        L10nKey::GitHubReviewCommented => "Commented",
        L10nKey::GitHubReviewRequested => "Requested",
        L10nKey::GitHubReadyToMerge => "Ready to merge",
        L10nKey::GitHubMergeConflicts => "Merge conflicts",
        L10nKey::GitHubReviewRequired => "Review required",
        L10nKey::GitHubBehindBase => "Behind the base branch",
        L10nKey::GitHubMergeBlocked => "Blocked by branch protection",
        L10nKey::GitHubThisBranch => "This branch",
        L10nKey::GitHubChecksFailing => "{count} checks failing",
        L10nKey::GitHubWaitingOnChecks => "Waiting on {count} checks",
        L10nKey::GitHubShowLess => "Show less",
        L10nKey::GitHubShowAllFiles => "Show all {count} files",
        L10nKey::GitHubPassedCount => "{count} passed",
        L10nKey::GitHubSkippedCount => "{count} skipped",
        L10nKey::GitHubShowFullText => "Show full text",
        L10nKey::GitHubShowHiddenComments => "Show {count} more comments",
        L10nKey::GitHubShowAllReviewers => "Show all {count} reviewers",
    }
}

pub fn translate_variant_en(key: L10nKey, branch: &'static str) -> Option<&'static str> {
    let res = match (key, branch) {
        (L10nKey::SettingsMatchCount, "zero") => "No matches",
        (L10nKey::SettingsMatchCount, "one") => "1 match",
        (L10nKey::SettingsMatchCount, "other") => "{count} matches",
        (L10nKey::PanelSearchResultCount, "zero") => "No results",
        (L10nKey::PanelSearchResultCount, "one") => "1 result",
        (L10nKey::PanelSearchResultCount, "other") => "{count} results",
        (L10nKey::PanelSearchFileCount, "zero") => "no files",
        (L10nKey::PanelSearchFileCount, "one") => "1 file",
        (L10nKey::PanelSearchFileCount, "other") => "{count} files",
        (L10nKey::SettingsRestoreChanged, "zero") => "Restore changes",
        (L10nKey::SettingsRestoreChanged, "one") => "Restore 1 changed",
        (L10nKey::SettingsRestoreChanged, "other") => "Restore {count} changed",
        (L10nKey::SettingsMoreHosts, "zero") => "No more hosts",
        (L10nKey::SettingsMoreHosts, "one") => "1 more host",
        (L10nKey::SettingsMoreHosts, "other") => "{count} more hosts",
        (L10nKey::SettingsMoreAgents, "zero") => "No more agents",
        (L10nKey::SettingsMoreAgents, "one") => "1 more agent available",
        (L10nKey::SettingsMoreAgents, "other") => "{count} more agents available",
        (L10nKey::SettingsAliasesLinked, "zero") => "No aliases linked yet.",
        (L10nKey::SettingsAliasesLinked, "one") => "1 alias linked.",
        (L10nKey::SettingsAliasesLinked, "other") => "{count} aliases linked.",
        (L10nKey::SettingsImportSummary, "zero") => {
            "Nothing new — {updated} updated, {unchanged} already current"
        }
        (L10nKey::SettingsImportSummary, "one") => {
            "1 host added — {updated} updated, {unchanged} already current"
        }
        (L10nKey::SettingsImportSummary, "other") => {
            "{count} hosts added — {updated} updated, {unchanged} already current"
        }
        (L10nKey::SettingsImportIgnored, "zero") => "Every option in the file has a lyrel setting.",
        (L10nKey::SettingsImportIgnored, "one") => {
            "1 option has no setting in lyrel and was left in the file: {options}"
        }
        (L10nKey::SettingsImportIgnored, "other") => {
            "{count} options have no setting in lyrel and were left in the file: {options}"
        }
        (L10nKey::SettingsRulesOpenedWithConnection, "zero") => {
            "0 rules, opened with the connection"
        }
        (L10nKey::SettingsRulesOpenedWithConnection, "one") => "1 rule, opened with the connection",
        (L10nKey::SettingsRulesOpenedWithConnection, "other") => {
            "{count} rules, opened with the connection"
        }
        (L10nKey::SettingsOfflineMachines, "zero") => {
            "0 more saved machines are not connected — open a workspace on one to install its hooks there."
        }
        (L10nKey::SettingsOfflineMachines, "one") => {
            "1 more saved machine is not connected — open a workspace on it to install its hooks there."
        }
        (L10nKey::SettingsOfflineMachines, "other") => {
            "{count} more saved machines are not connected — open a workspace on one to install its hooks there."
        }
        (L10nKey::SettingsForgetPasswordSharedBody, "one") => {
            "1 other host profile uses {endpoint} as well, so that connection will have to enter the password again too."
        }
        (L10nKey::SettingsForgetPasswordSharedBody, "other") => {
            "{count} other host profiles use {endpoint} as well, so those connections will have to enter the password again too."
        }
        (L10nKey::SettingsDeleteProfileCascade, "one") => {
            "1 saved remote workspace entry points at {endpoint} and is removed from \
             this computer along with it — the session on the remote machine keeps \
             running, and connecting with a new profile brings it back to the \
             workspace list."
        }
        (L10nKey::SettingsDeleteProfileCascade, "other") => {
            "{count} saved remote workspace entries point at {endpoint} and are removed from \
             this computer along with it — the sessions on the remote machine keep running, \
             and connecting with a new profile brings them back to the workspace list."
        }
        (L10nKey::SftpReplaceBody, "one") => {
            "{names} already exists in this folder. Uploading overwrites it."
        }
        (L10nKey::SftpReplaceBody, "other") => {
            "{names} already exist in this folder. Uploading overwrites them."
        }
        (L10nKey::CloseTabsTitle, "one") => "Close 1 tab?",
        (L10nKey::CloseTabsTitle, "other") => "Close {count} tabs?",
        (L10nKey::AppTabsNotRestored, "one") => "1 tab from last time could not be reopened",
        (L10nKey::AppTabsNotRestored, "other") => {
            "{count} tabs from last time could not be reopened"
        }
        (L10nKey::LaunchWorkspacesLeftRunning, "one") => {
            "Only this window was restored — 1 workspace is still running in the background. Reopen it from the sidebar."
        }
        (L10nKey::LaunchWorkspacesLeftRunning, "other") => {
            "Only this window was restored — {count} workspaces are still running in the background. Reopen them from the sidebar."
        }
        (L10nKey::ScmFilesChanged, "zero") => "No files changed",
        (L10nKey::ScmFilesChanged, "one") => "1 file changed",
        (L10nKey::ScmFilesChanged, "other") => "{count} files changed",
        (L10nKey::ScmStagedFileCount, "zero") => "No staged changes",
        (L10nKey::ScmStagedFileCount, "one") => "1 file staged",
        (L10nKey::ScmStagedFileCount, "other") => "{count} files staged",
        (L10nKey::PanelMoreChangedFiles, "zero") => {
            "… and 0 more changed files — run git diff to see them."
        }
        (L10nKey::PanelMoreChangedFiles, "one") => {
            "… and 1 more changed file — run git diff to see it."
        }
        (L10nKey::PanelMoreChangedFiles, "other") => {
            "… and {count} more changed files — run git diff to see them."
        }
        (L10nKey::DiffChangedFiles, "zero") => "0 changed files",
        (L10nKey::DiffChangedFiles, "one") => "1 changed file",
        (L10nKey::DiffChangedFiles, "other") => "{count} changed files",
        (L10nKey::DiffUntrackedCount, "zero") => " · 0 untracked",
        (L10nKey::DiffUntrackedCount, "one") => " · 1 untracked",
        (L10nKey::DiffUntrackedCount, "other") => " · {count} untracked",
        (L10nKey::DiffMoreFiles, "zero") => {
            "… and 0 more changed files — run git diff in the terminal to see them."
        }
        (L10nKey::DiffMoreFiles, "one") => {
            "… and 1 more changed file — run git diff in the terminal to see it."
        }
        (L10nKey::DiffMoreFiles, "other") => {
            "… and {count} more changed files — run git diff in the terminal to see them."
        }
        (L10nKey::DiffUntrackedHeader, "zero") => "Untracked files (0)",
        (L10nKey::DiffUntrackedHeader, "one") => "Untracked files (1)",
        (L10nKey::DiffUntrackedHeader, "other") => "Untracked files ({count})",
        (L10nKey::DiffMoreUntracked, "zero") => {
            "… and 0 more — run git status in the terminal to see them."
        }
        (L10nKey::DiffMoreUntracked, "one") => {
            "… and 1 more — run git status in the terminal to see it."
        }
        (L10nKey::DiffMoreUntracked, "other") => {
            "… and {count} more — run git status in the terminal to see them."
        }
        (L10nKey::DiffUntrackedSummary, "zero") => "0 untracked",
        (L10nKey::DiffUntrackedSummary, "one") => "1 untracked",
        (L10nKey::DiffUntrackedSummary, "other") => "{count} untracked",
        (L10nKey::HomeTimeMinutesAgo, "one") => "1 min ago",
        (L10nKey::HomeTimeMinutesAgo, "other") => "{count} min ago",
        (L10nKey::HomeTimeHoursAgo, "one") => "1 hour ago",
        (L10nKey::HomeTimeHoursAgo, "other") => "{count} hours ago",
        (L10nKey::HomeTimeDaysAgo, "one") => "1 day ago",
        (L10nKey::HomeTimeDaysAgo, "other") => "{count} days ago",
        (L10nKey::HomeTimeWeeksAgo, "one") => "1 week ago",
        (L10nKey::HomeTimeWeeksAgo, "other") => "{count} weeks ago",
        (L10nKey::HomeTimeMonthsAgo, "one") => "1 month ago",
        (L10nKey::HomeTimeMonthsAgo, "other") => "{count} months ago",
        (L10nKey::WindowStopShells, "zero") => {
            "Its layout and working directories will be forgotten."
        }
        (L10nKey::WindowStopShells, "one") => "1 running shell will be ended.",
        (L10nKey::WindowStopShells, "other") => "{count} running shells will be ended.",
        (L10nKey::WindowDeleteShells, "zero") => {
            "Its layout and working directories will be forgotten."
        }
        (L10nKey::WindowDeleteShells, "one") => {
            "1 running shell will be ended and its layout forgotten."
        }
        (L10nKey::WindowDeleteShells, "other") => {
            "{count} running shells will be ended and the layout forgotten."
        }
        // ---- The right panel's GitHub tab ----
        (L10nKey::GitHubComments, "zero") => "No comments",
        (L10nKey::GitHubComments, "one") => "1 comment",
        (L10nKey::GitHubComments, "other") => "{count} comments",
        (L10nKey::GitHubCommits, "zero") => "No commits",
        (L10nKey::GitHubCommits, "one") => "1 commit",
        (L10nKey::GitHubCommits, "other") => "{count} commits",
        (L10nKey::GitHubChecksFailing, "zero") => "No checks failing",
        (L10nKey::GitHubChecksFailing, "one") => "1 check failing",
        (L10nKey::GitHubChecksFailing, "other") => "{count} checks failing",
        (L10nKey::GitHubWaitingOnChecks, "zero") => "Not waiting on checks",
        (L10nKey::GitHubWaitingOnChecks, "one") => "Waiting on 1 check",
        (L10nKey::GitHubWaitingOnChecks, "other") => "Waiting on {count} checks",
        (L10nKey::GitHubShowAllFiles, "zero") => "No files",
        (L10nKey::GitHubShowAllFiles, "one") => "Show 1 file",
        (L10nKey::GitHubShowAllFiles, "other") => "Show all {count} files",
        (L10nKey::GitHubPassedCount, "zero") => "None passed",
        (L10nKey::GitHubPassedCount, "one") => "1 passed",
        (L10nKey::GitHubPassedCount, "other") => "{count} passed",
        (L10nKey::GitHubSkippedCount, "zero") => "None skipped",
        (L10nKey::GitHubSkippedCount, "one") => "1 skipped",
        (L10nKey::GitHubSkippedCount, "other") => "{count} skipped",
        (L10nKey::GitHubShowHiddenComments, "zero") => "No more comments",
        (L10nKey::GitHubShowHiddenComments, "one") => "Show 1 more comment",
        (L10nKey::GitHubShowHiddenComments, "other") => "Show {count} more comments",
        (L10nKey::GitHubShowAllReviewers, "zero") => "No reviewers",
        (L10nKey::GitHubShowAllReviewers, "one") => "Show 1 reviewer",
        (L10nKey::GitHubShowAllReviewers, "other") => "Show all {count} reviewers",
        _ => return None,
    };
    Some(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_is_the_complete_table() {
        assert_eq!(
            translate_variant_en(L10nKey::SettingsAliasesLinked, "one"),
            Some("1 alias linked.")
        );
    }
}
