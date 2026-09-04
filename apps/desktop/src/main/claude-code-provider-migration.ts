import {
	extractClaudeCodeProviderSettings,
	formatClaudeCodeProviderSettingsPreview,
} from "@devo/configconv"
import type { ClaudeCodeProviderSettings, ClaudeSettings } from "@devo/configconv"
import type { CanonicalProviderUpsertParams } from "./canonical-provider-migration"

interface MigrationFilePreview {
	path: string
	status: "new" | "modified" | "skipped"
	lineCount: number
	content?: string
}

interface MigrationCategoryPreview {
	category: string
	itemCount: number
	files: MigrationFilePreview[]
}

interface ClaudeCodeProviderMigrationResult {
	category: MigrationCategoryPreview | null
	warnings: string[]
	manualActions: string[]
	errors: string[]
}

interface ClaudeCodeProviderMigrationWriteResult {
	filesWritten: string[]
	warnings: string[]
	manualActions: string[]
	errors: string[]
}

type RequestProviderUpsert = (method: string, params?: unknown) => Promise<unknown>

export function buildClaudeCodeProviderMigrationPreview(
	scanResult: unknown,
): ClaudeCodeProviderMigrationResult {
	const settings = extractClaudeCodeProviderSettings(readClaudeCodeSettings(scanResult))
	const diagnostics = diagnosticsFor(settings)
	if (settings.models.length === 0) {
		return {
			category: null,
			...diagnostics,
		}
	}

	const content = formatClaudeCodeProviderSettingsPreview(settings)
	return {
		category: {
			category: "config",
			itemCount: settings.models.length,
			files: [
				{
					path: `provider/upsert:${settings.providerId}`,
					status: "new",
					lineCount: content.split("\n").length,
					content,
				},
			],
		},
		...diagnostics,
	}
}

export async function executeClaudeCodeProviderMigration(
	scanResult: unknown,
	requestProviderUpsert: RequestProviderUpsert,
): Promise<ClaudeCodeProviderMigrationWriteResult> {
	const settings = extractClaudeCodeProviderSettings(readClaudeCodeSettings(scanResult))
	const diagnostics = diagnosticsFor(settings)
	const filesWritten: string[] = []
	const errors: string[] = [...diagnostics.errors]

	for (const params of buildProviderUpsertParams(settings)) {
		try {
			await requestProviderUpsert("provider/upsert", params)
			filesWritten.push(`provider/upsert:${settings.providerId}`)
		} catch (error) {
			errors.push(
				`Claude Code provider migration failed for ${settings.providerId}: ${error instanceof Error ? error.message : String(error)}`,
			)
		}
	}

	return {
		filesWritten,
		warnings: diagnostics.warnings,
		manualActions: diagnostics.manualActions,
		errors,
	}
}

function buildProviderUpsertParams(
	settings: ClaudeCodeProviderSettings,
): CanonicalProviderUpsertParams[] {
	if (settings.models.length === 0) return []

	return [
		{
			provider: {
				id: settings.providerId,
				name: settings.providerId,
				...(settings.baseUrl ? { baseUrl: settings.baseUrl } : {}),
				wireApis: [settings.wireApi],
				models: Object.fromEntries(settings.models.map((model) => [model, { name: model }])),
				enabled: true,
			},
			...(settings.defaultModel
				? { defaultModel: `${settings.providerId}/${settings.defaultModel}` }
				: {}),
			...(settings.apiKey ? { apiKey: settings.apiKey } : {}),
		},
	]
}

function diagnosticsFor(settings: ClaudeCodeProviderSettings): {
	warnings: string[]
	manualActions: string[]
	errors: string[]
} {
	const warnings: string[] = []
	const manualActions: string[] = []

	if (settings.models.length === 0) {
		warnings.push(
			"Claude Code settings did not include ANTHROPIC_MODEL or default Anthropic model env vars; no provider Connection was imported.",
		)
	}
	if (!settings.apiKey) {
		manualActions.push(
			"Claude Code settings did not include ANTHROPIC_AUTH_TOKEN or ANTHROPIC_API_KEY. Add an API key manually after migration.",
		)
	}

	return { warnings, manualActions, errors: [] }
}

function readClaudeCodeSettings(scanResult: unknown): ClaudeSettings | undefined {
	if (!isRecord(scanResult)) return undefined
	const data = scanResult.data
	if (!isRecord(data)) return undefined
	const global = data.global
	if (!isRecord(global)) return undefined
	return isRecord(global.settings) ? (global.settings as ClaudeSettings) : undefined
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value)
}
