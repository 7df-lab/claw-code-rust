export interface QueueWireEntry {
	queueItemId: string
	position: number
	preview: string
	enqueuedAt?: string
	input?: Array<{ type: string; text?: string }>
}

export function queueEntryText(entry: QueueWireEntry): string {
	const texts =
		entry.input
			?.filter((part) => part.type === "text" && part.text)
			.map((part) => part.text as string) ?? []
	if (texts.length > 0) return texts.join("\n")
	return entry.preview
}

export function queueRenderPreview(text: string): string {
	return text.split(/\s+/).filter(Boolean).join(" ")
}

export function countQueueFileParts(entry: QueueWireEntry): number {
	return (
		entry.input?.filter((part) => part.type !== "text" && part.type !== "skill").length ?? 0
	)
}
