/**
 * IPC envelope for expected Native protocol errors that must not reject
 * `ipcMain.handle` (Electron treats handler rejections as unhandled briefly
 * and logs "Error occurred in handler").
 *
 * The renderer IPC transport converts this envelope back into a thrown Error
 * before the SDK sees the result.
 */
export const DEVO_NATIVE_IPC_ERROR = "$devoNativeError" as const

export type DevoNativeIpcErrorEnvelope = {
	[DEVO_NATIVE_IPC_ERROR]: {
		message: string
		code?: string
	}
}

export function isSessionNotFoundError(error: unknown): boolean {
	if (!error) return false
	const record = typeof error === "object" && error !== null ? (error as Record<string, unknown>) : null
	const code = typeof record?.code === "string" ? record.code : undefined
	if (code === "SessionNotFound" || code === "session_not_found") return true
	const message = error instanceof Error ? error.message : String(error)
	return (
		/session does not exist/i.test(message) ||
		/^session .+ not found$/i.test(message) ||
		/session id is not addressable by this server/i.test(message)
	)
}

export function nativeIpcErrorEnvelope(error: unknown): DevoNativeIpcErrorEnvelope {
	const record = typeof error === "object" && error !== null ? (error as Record<string, unknown>) : null
	return {
		[DEVO_NATIVE_IPC_ERROR]: {
			message: error instanceof Error ? error.message : String(error),
			code: typeof record?.code === "string" ? record.code : undefined,
		},
	}
}

export function errorFromNativeIpcEnvelope(value: unknown): Error | null {
	if (!value || typeof value !== "object") return null
	const envelope = (value as Record<string, unknown>)[DEVO_NATIVE_IPC_ERROR]
	if (!envelope || typeof envelope !== "object") return null
	const record = envelope as Record<string, unknown>
	const message = typeof record.message === "string" ? record.message : "Devo Native request failed"
	const error = new Error(message)
	if (typeof record.code === "string") {
		;(error as Error & { code?: string }).code = record.code
	}
	return error
}
