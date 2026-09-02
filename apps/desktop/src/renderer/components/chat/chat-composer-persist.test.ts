import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const chatViewSource = readFileSync(new URL("./chat-view.tsx", import.meta.url), "utf8")
	.replace(/\r\n/g, "\n")
const clientSource = readFileSync(
	new URL("../../../../packages/devo-ai-sdk/src/v2/client.ts", import.meta.url),
	"utf8",
).replace(/\r\n/g, "\n")
const newChatSource = readFileSync(new URL("../new-chat.tsx", import.meta.url), "utf8").replace(
	/\r\n/g,
	"\n",
)

/**
 * Persist-on-selection: changing model / reasoning effort / mode in the
 * composer must reach the server immediately (debounced, coalesced), not
 * only when the next message is sent — otherwise unsent selections are lost
 * on restart.
 */
describe("composer persist-on-selection", () => {
	test("chat-view schedules a debounced persist on every selection change", () => {
		expect({
			debounceConstant: chatViewSource.includes("SELECTION_PERSIST_DEBOUNCE_MS"),
			scheduler: chatViewSource.includes("scheduleSelectionPersist({"),
			persistsModel: chatViewSource.includes("scheduleSelectionPersist({ modelID: model.modelID })"),
			persistsEffort: chatViewSource.includes("scheduleSelectionPersist({ reasoningEffort: variant })"),
			persistsMode: chatViewSource.includes('scheduleSelectionPersist({ mode: next })'),
			persistsPermission: chatViewSource.includes(
				"scheduleSelectionPersist({ permissionProfile: profile })",
			),
			flushesOnUnmount: chatViewSource.includes("return () => {\n\t\t\tflushSelectionPersist()\n\t\t}"),
			usesCombinedUpdate: chatViewSource.includes("updateSettings.call(client.session"),
			showsSaveStatus: chatViewSource.includes("Saving session settings") || chatViewSource.includes("Session settings saved"),
			showsFailureOrRetryStatus:
				chatViewSource.includes("Session settings could not be saved") ||
				chatViewSource.includes("retrySelectionPersist"),
			reportsPersistFailureToLogger:
				chatViewSource.includes("Promise.resolve()") &&
				chatViewSource.includes('log.warn("session settings persist failed"'),
		}).toEqual({
			debounceConstant: true,
			scheduler: true,
			persistsModel: true,
			persistsEffort: true,
			persistsMode: true,
			persistsPermission: true,
			flushesOnUnmount: true,
			usesCombinedUpdate: true,
			showsSaveStatus: false,
			showsFailureOrRetryStatus: false,
			reportsPersistFailureToLogger: true,
		})
	})

	test("mode toggles route through the persisting wrapper", () => {
		expect(chatViewSource.includes("changeCollaborationMode")).toBe(true)
		expect(chatViewSource.includes("setCollaborationMode(\"")).toBe(false)
	})

	test("SDK exposes session.updateSettings with a combined metadata patch", () => {
		expect(clientSource.includes("updateSettings: async (params")).toBe(true)
		expect(clientSource.includes('"session/metadata/update"')).toBe(true)
	})

	test("SDK persist patch can include a permission profile", () => {
		expect(clientSource.includes("if (patch.permissionProfile) settings.permissionProfile")).toBe(
			true,
		)
		expect(clientSource.includes("normalizedPatch.permissionProfile = patch.permissionProfile")).toBe(
			true,
		)
	})

	/**
	 * Turn-start must not re-derive persisted selections: callers used to
	 * pass fallback-resolved models (request slugs, defaults) which the old
	 * auto-enqueue wrote over the user's persisted choice on every send.
	 * Only the collaboration mode still rides along (canonical turn/start
	 * carries no mode).
	 */
	test("SDK turn.start persists only the collaboration mode, not model/variant", () => {
		expect(clientSource.includes("...(model?.modelID ? { modelID: model.modelID } : {})")).toBe(false)
		expect(
			clientSource.includes("...(params.variant ? { reasoningEffort: params.variant } : {})"),
		).toBe(false)
		const turnStart = clientSource.slice(
			clientSource.indexOf("turn = {"),
			clientSource.indexOf("task = {"),
		)
		expect(turnStart.includes("mode: params.collaborationMode")).toBe(true)
		expect(turnStart.includes("enqueueSessionSettings")).toBe(true)
		expect(turnStart.includes("modelID")).toBe(false)
	})

	/** Sends carry only the explicit composer selection, flushed first. */
	test("chat-view sends the explicit selection after flushing pending persists", () => {
		expect(
			chatViewSource.includes(
				"? { providerID: selectedModel.providerID, modelID: selectedModel.modelID }",
			),
		).toBe(true)
		expect(chatViewSource.includes("model: selectedModel ?? undefined")).toBe(true)
		expect(chatViewSource.includes("model: effectiveModel ?? undefined")).toBe(false)
		expect(chatViewSource.includes("await flushSelectionPersist()")).toBe(true)
		expect(
			chatViewSource.includes("const flushSelectionPersist = useCallback((): Promise<void> =>"),
		).toBe(true)
	})

	/**
	 * Hydration must stay retryable: the guard key is recorded only once the
	 * seed actually resolved, so a provider list that arrives after the first
	 * effect run can still seed the composer instead of leaving defaults.
	 */
	test("hydrate guard records the key only after a conclusive hydration", () => {
		expect(
			chatViewSource.includes(
				"if (next.model || !wireSession?.model?.model || composerState.hasUserOverride) {",
			),
		).toBe(true)
		expect(
			chatViewSource.includes(
				"if (hydratedForMessagesRef.current === messageKey) return\n\t\thydratedForMessagesRef.current = messageKey",
			),
		).toBe(false)
	})

	test("new-session composer persists permission and plan onto the created session", () => {
		expect(newChatSource.includes("persistLaunchSettings")).toBe(true)
		expect(newChatSource.includes("permissionProfile,")).toBe(true)
		expect(newChatSource.includes("mode: collaborationMode")).toBe(true)
		expect(newChatSource.includes("DEFAULT_COMPOSER_PERMISSION_PROFILE")).toBe(true)
	})
})
