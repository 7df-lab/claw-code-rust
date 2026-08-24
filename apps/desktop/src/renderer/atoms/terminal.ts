import { atom } from "jotai"

export const terminalPanelOpenAtom = atom(false)

export const terminalNewTabNonceAtom = atom(0)

export const openNewTerminalAtom = atom(null, (get, set) => {
	const wasOpen = get(terminalPanelOpenAtom)
	set(terminalPanelOpenAtom, true)
	if (wasOpen) {
		set(terminalNewTabNonceAtom, get(terminalNewTabNonceAtom) + 1)
	}
})
