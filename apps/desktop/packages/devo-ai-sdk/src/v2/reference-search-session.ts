/**
 * Connection-local composer `@` reference search session.
 *
 * `@` mention file search uses `search/start`, `search/update`, and `search/cancel`
 * RPCs plus `search/updated` / `search/completed` notifications — not a separate
 * `find.files` endpoint. See L2-DES-APP-003 Search Protocol Rules.
 *
 * The server owns fuzzy ranking; clients render snapshots and ignore stale
 * `query` / `search_id` pairs when the popup query has already moved on.
 */

export type ReferenceSearchResultKind = "skill" | "mcp" | "file"

export interface ReferenceSearchResult {
	kind: ReferenceSearchResultKind
	display_name: string
	insert_text: string
	description?: string
	mention_path?: string
	file_path?: string
	is_disabled?: boolean
	disabled_reason?: string
}

export interface ReferenceSearchSnapshot {
	search_id: string
	query: string
	results: ReferenceSearchResult[]
	total_file_match_count: number
	scanned_file_count: number
	file_search_complete: boolean
}

export interface ReferenceSearchFailedPayload {
	search_id: string
	query: string
	message: string
}

type SearchRequest = (method: string, params: unknown) => Promise<unknown>

type SnapshotListener = (snapshot: ReferenceSearchSnapshot) => void

type ReferenceSearchState = {
	snapshot: ReferenceSearchSnapshot | null
	error: string | null
}

function parseReferenceResult(value: unknown): ReferenceSearchResult | null {
	if (!value || typeof value !== "object") return null
	const raw = value as Record<string, unknown>
	const kind = raw.kind
	if (kind !== "skill" && kind !== "mcp" && kind !== "file") return null
	if (typeof raw.display_name !== "string" && typeof raw.displayName !== "string") return null
	if (typeof raw.insert_text !== "string" && typeof raw.insertText !== "string") return null

	const isDisabled = raw.is_disabled ?? raw.isDisabled
	const disabledReason = raw.disabled_reason ?? raw.disabledReason
	return {
		kind,
		display_name: (raw.display_name ?? raw.displayName) as string,
		insert_text: (raw.insert_text ?? raw.insertText) as string,
		description:
			typeof raw.description === "string"
				? raw.description
				: undefined,
		mention_path:
			typeof (raw.mention_path ?? raw.mentionPath) === "string"
				? ((raw.mention_path ?? raw.mentionPath) as string)
				: undefined,
		file_path:
			typeof (raw.file_path ?? raw.filePath) === "string"
				? ((raw.file_path ?? raw.filePath) as string)
				: undefined,
		is_disabled: isDisabled === true,
		disabled_reason: typeof disabledReason === "string" ? disabledReason : undefined,
	}
}

function parseSnapshot(payload: unknown): ReferenceSearchSnapshot | null {
	if (!payload || typeof payload !== "object") return null
	if ("snapshot" in payload) {
		const snapshot = (payload as { snapshot?: unknown }).snapshot
		return parseSnapshot(snapshot)
	}
	const candidate = payload as Partial<ReferenceSearchSnapshot> & Record<string, unknown>
	const searchId = candidate.search_id ?? candidate.searchId
	const totalFileMatchCount = candidate.total_file_match_count ?? candidate.totalFileMatchCount
	const scannedFileCount = candidate.scanned_file_count ?? candidate.scannedFileCount
	const fileSearchComplete = candidate.file_search_complete ?? candidate.fileSearchComplete
	if (typeof searchId !== "string" || typeof candidate.query !== "string") {
		return null
	}
	const results = Array.isArray(candidate.results)
		? candidate.results
				.map(parseReferenceResult)
				.filter((result): result is ReferenceSearchResult => result != null)
		: []
	return {
		search_id: searchId,
		query: candidate.query,
		results,
		total_file_match_count: typeof totalFileMatchCount === "number" ? totalFileMatchCount : 0,
		scanned_file_count: typeof scannedFileCount === "number" ? scannedFileCount : 0,
		file_search_complete: typeof fileSearchComplete === "boolean" ? fileSearchComplete : false,
	}
}

function filePathsFromSnapshot(snapshot: ReferenceSearchSnapshot): string[] {
	return snapshot.results
		.filter((result) => result.kind === "file")
		.map((result) => result.display_name)
		.filter((path) => path.trim().length > 0)
}

export class ReferenceSearchSession {
	private searchId: string | null = null
	private activeQuery = ""
	private snapshot: ReferenceSearchSnapshot | null = null
	private error: string | null = null
	private readonly listeners = new Set<SnapshotListener>()

	constructor(
		private readonly request: SearchRequest,
		private readonly cwd: string,
	) {}

	subscribe(listener: SnapshotListener): () => void {
		this.listeners.add(listener)
		if (this.snapshot) listener(this.snapshot)
		return () => {
			this.listeners.delete(listener)
		}
	}

	getState(): ReferenceSearchState {
		return { snapshot: this.snapshot, error: this.error }
	}

	filePaths(): string[] {
		return this.snapshot ? filePathsFromSnapshot(this.snapshot) : []
	}

	async startOrUpdate(query: string): Promise<ReferenceSearchSnapshot> {
		this.activeQuery = query
		this.error = null
		const snapshot = this.searchId
			? await this.update(query)
			: await this.start(query)
		this.applySnapshot(snapshot)
		return snapshot
	}

	async cancel(): Promise<void> {
		const searchId = this.searchId
		this.searchId = null
		this.activeQuery = ""
		this.snapshot = null
		this.error = null
		if (!searchId) return
		try {
			await this.request("search/cancel", { searchId })
		} catch {
			// Best-effort cleanup when the popup closes.
		}
	}

	handleNotification(method: string, payload: unknown): boolean {
		const innerMethod = method
		if (innerMethod === "search/failed") {
			const failed = payload as Partial<ReferenceSearchFailedPayload>
			if (!failed.search_id || this.searchId !== failed.search_id) return true
			if (failed.query && failed.query !== this.activeQuery) return true
			this.error = failed.message ?? "reference search failed"
			if (this.snapshot) {
				this.applySnapshot({
					...this.snapshot,
					file_search_complete: true,
				})
			}
			return true
		}
		if (innerMethod !== "search/updated" && innerMethod !== "search/completed") {
			return false
		}
		const snapshot = parseSnapshot(payload)
		if (!snapshot) return true
		if (this.searchId && snapshot.search_id !== this.searchId) return true
		if (snapshot.query !== this.activeQuery) return true
		this.applySnapshot(snapshot)
		return true
	}

	private async start(query: string): Promise<ReferenceSearchSnapshot> {
		const result = (await this.request("search/start", {
			cwd: this.cwd,
			query,
		})) as { snapshot: ReferenceSearchSnapshot }
		const snapshot = parseSnapshot(result.snapshot)
		if (!snapshot) throw new Error("invalid reference search start response")
		this.searchId = snapshot.search_id
		return snapshot
	}

	private async update(query: string): Promise<ReferenceSearchSnapshot> {
		const result = (await this.request("search/update", {
			searchId: this.searchId,
			query,
		})) as { snapshot: ReferenceSearchSnapshot }
		const snapshot = parseSnapshot(result.snapshot)
		if (!snapshot) throw new Error("invalid reference search update response")
		return snapshot
	}

	private applySnapshot(snapshot: ReferenceSearchSnapshot): void {
		this.snapshot = snapshot
		for (const listener of this.listeners) {
			listener(snapshot)
		}
	}
}
