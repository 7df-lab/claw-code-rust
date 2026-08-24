/** Normalize a filesystem path for equality checks across Windows and Unix. */
export function normalizeDirectoryPath(value: string): string {
	return value.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase()
}

export function directoriesMatch(left: string, right: string): boolean {
	return normalizeDirectoryPath(left) === normalizeDirectoryPath(right)
}
