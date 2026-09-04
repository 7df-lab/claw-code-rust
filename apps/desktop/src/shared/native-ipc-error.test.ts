import { describe, expect, test } from "bun:test"
import {
	DEVO_NATIVE_IPC_ERROR,
	errorFromNativeIpcEnvelope,
	isSessionNotFoundError,
	nativeIpcErrorEnvelope,
} from "../../src/shared/native-ipc-error"

describe("native-ipc-error", () => {
	test("detects SessionNotFound by message and code", () => {
		expect(isSessionNotFoundError(new Error("session does not exist"))).toBe(true)
		const coded = new Error("missing") as Error & { code?: string }
		coded.code = "SessionNotFound"
		expect(isSessionNotFoundError(coded)).toBe(true)
		expect(isSessionNotFoundError(new Error("network"))).toBe(false)
	})

	test("round-trips IPC error envelopes", () => {
		const coded = new Error("session does not exist") as Error & { code?: string }
		coded.code = "SessionNotFound"
		const envelope = nativeIpcErrorEnvelope(coded)
		expect(envelope[DEVO_NATIVE_IPC_ERROR]?.code).toBe("SessionNotFound")
		const restored = errorFromNativeIpcEnvelope(envelope)
		expect(restored).toBeInstanceOf(Error)
		expect(restored?.message).toBe("session does not exist")
		expect((restored as Error & { code?: string }).code).toBe("SessionNotFound")
		expect(isSessionNotFoundError(restored)).toBe(true)
	})
})
