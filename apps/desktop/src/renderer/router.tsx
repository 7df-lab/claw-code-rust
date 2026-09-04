import {
	createHashHistory,
	createRootRoute,
	createRoute,
	createRouter,
	redirect,
} from "@tanstack/react-router"
import { AutomationDetail } from "./components/automations/automation-detail"
import { AutomationRunDetail } from "./components/automations/automation-run-detail"
import { AutomationsPage } from "./components/automations/automations-page"
import { InboxEmptyState } from "./components/automations/inbox-empty-state"
import { ErrorPage } from "./components/error-page"
import { NewChat } from "./components/new-chat"
import { NotFoundPage } from "./components/not-found-page"
import { RootLayout } from "./components/root-layout"
import { SessionRoute } from "./components/session-route"
import { AboutSettings } from "./components/settings/about-settings"
import { GeneralSettings } from "./components/settings/general-settings"
import { McpSettings } from "./components/settings/mcp-settings"
import { NotificationSettings } from "./components/settings/notification-settings"
import { ModelSettings } from "./components/settings/model-settings"
import { ProviderSettings } from "./components/settings/provider-settings"
import { ServerSettings } from "./components/settings/server-settings"
import { SettingsPage } from "./components/settings/settings-page"
import { SetupSettings } from "./components/settings/setup-settings"
import { SkillSettings } from "./components/settings/skill-settings"
import { WorktreeSettings } from "./components/settings/worktree-settings"
import { SidebarLayout } from "./components/sidebar-layout"

// ============================================================
// Route tree
// ============================================================

const rootRoute = createRootRoute({
	component: RootLayout,
	errorComponent: ErrorPage,
	notFoundComponent: NotFoundPage,
})

const sidebarLayout = createRoute({
	getParentRoute: () => rootRoute,
	id: "sidebar",
	component: SidebarLayout,
})

const indexRoute = createRoute({
	getParentRoute: () => sidebarLayout,
	path: "/",
	component: NewChat,
})

const projectRoute = createRoute({
	getParentRoute: () => sidebarLayout,
	path: "project/$projectSlug",
})

const projectIndexRoute = createRoute({
	getParentRoute: () => projectRoute,
	path: "/",
	component: NewChat,
})

const sessionRoute = createRoute({
	getParentRoute: () => projectRoute,
	path: "session/$sessionId",
	component: SessionRoute,
})

const settingsRoute = createRoute({
	getParentRoute: () => sidebarLayout,
	path: "settings",
	component: SettingsPage,
})

const settingsIndexRoute = createRoute({
	getParentRoute: () => settingsRoute,
	path: "/",
	beforeLoad: () => {
		throw redirect({ to: "/settings/general" })
	},
})

const settingsGeneralRoute = createRoute({
	getParentRoute: () => settingsRoute,
	path: "general",
	component: GeneralSettings,
})

const settingsServersRoute = createRoute({
	getParentRoute: () => settingsRoute,
	path: "servers",
	component: ServerSettings,
})

const settingsMcpRoute = createRoute({
	getParentRoute: () => settingsRoute,
	path: "mcp",
	component: McpSettings,
})

const settingsSkillsRoute = createRoute({
	getParentRoute: () => settingsRoute,
	path: "skills",
	component: SkillSettings,
})

const settingsNotificationsRoute = createRoute({
	getParentRoute: () => settingsRoute,
	path: "notifications",
	component: NotificationSettings,
})

const settingsSetupRoute = createRoute({
	getParentRoute: () => settingsRoute,
	path: "setup",
	component: SetupSettings,
})

const settingsProvidersRoute = createRoute({
	getParentRoute: () => settingsRoute,
	path: "providers",
	component: ProviderSettings,
})

const settingsModelsRoute = createRoute({
	getParentRoute: () => settingsRoute,
	path: "models",
	component: ModelSettings,
})

const settingsWorktreesRoute = createRoute({
	getParentRoute: () => settingsRoute,
	path: "worktrees",
	component: WorktreeSettings,
})

const settingsAboutRoute = createRoute({
	getParentRoute: () => settingsRoute,
	path: "about",
	component: AboutSettings,
})

const automationsRoute = createRoute({
	getParentRoute: () => sidebarLayout,
	path: "automations",
	component: AutomationsPage,
})

const automationsIndexRoute = createRoute({
	getParentRoute: () => automationsRoute,
	path: "/",
	component: InboxEmptyState,
})

const automationDetailRoute = createRoute({
	getParentRoute: () => automationsRoute,
	path: "$automationId",
})

const automationDetailIndexRoute = createRoute({
	getParentRoute: () => automationDetailRoute,
	path: "/",
	component: AutomationDetail,
})

const automationRunRoute = createRoute({
	getParentRoute: () => automationDetailRoute,
	path: "runs/$runId",
	component: AutomationRunDetail,
})

const routeTree = rootRoute.addChildren([
	sidebarLayout.addChildren([
		indexRoute,
		projectRoute.addChildren([projectIndexRoute, sessionRoute]),
		automationsRoute.addChildren([
			automationsIndexRoute,
			automationDetailRoute.addChildren([automationDetailIndexRoute, automationRunRoute]),
		]),
		settingsRoute.addChildren([
			settingsIndexRoute,
			settingsGeneralRoute,
			settingsServersRoute,
			settingsProvidersRoute,
			settingsModelsRoute,
			settingsMcpRoute,
			settingsSkillsRoute,
			settingsNotificationsRoute,
			settingsWorktreesRoute,
			settingsSetupRoute,
			settingsAboutRoute,
		]),
	]),
])

// ============================================================
// Router instance
// ============================================================

const hashHistory = createHashHistory()

export const router = createRouter({
	routeTree,
	history: hashHistory,
	defaultErrorComponent: ErrorPage,
	defaultNotFoundComponent: NotFoundPage,
})

export type AppRouter = typeof router

// ============================================================
// Type-safe module augmentation
// ============================================================

declare module "@tanstack/react-router" {
	interface Register {
		router: typeof router
	}
}
