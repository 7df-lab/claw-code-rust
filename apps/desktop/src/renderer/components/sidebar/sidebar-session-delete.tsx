import { Button } from "@devo/ui/components/button"
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@devo/ui/components/dialog"
import { Loader2Icon } from "lucide-react"

export type DeleteSessionNavigationTarget =
	| {
			to: "/project/$projectSlug"
			params: { projectSlug: string }
	  }
	| { to: "/" }
	| null

export function deleteSessionNavigationTarget({
	deletedSessionId,
	currentSessionId,
	projectSlug,
}: {
	deletedSessionId: string
	currentSessionId: string | null | undefined
	projectSlug: string | undefined
}): DeleteSessionNavigationTarget {
	if (!currentSessionId || deletedSessionId !== currentSessionId) return null
	if (projectSlug) {
		return {
			to: "/project/$projectSlug",
			params: { projectSlug },
		}
	}
	return { to: "/" }
}

export function SessionDeleteDialog({
	open,
	pending,
	error,
	onOpenChange,
	onConfirm,
}: {
	open: boolean
	pending: boolean
	error: string | null
	onOpenChange: (open: boolean) => void
	onConfirm: () => void
}) {
	return (
		<Dialog open={open} onOpenChange={(isOpen) => !pending && onOpenChange(isOpen)}>
			<DialogContent showCloseButton={false} className="gap-4 sm:max-w-md">
				<SessionDeleteDialogBody
					pending={pending}
					error={error}
					onCancel={() => onOpenChange(false)}
					onConfirm={onConfirm}
				/>
			</DialogContent>
		</Dialog>
	)
}

export function SessionDeleteDialogBody({
	pending,
	error,
	onCancel,
	onConfirm,
}: {
	pending: boolean
	error: string | null
	onCancel: () => void
	onConfirm: () => void
}) {
	return (
		<>
			<DialogHeader>
				<DialogTitle>Delete session</DialogTitle>
				<DialogDescription>
					This permanently removes the session and its history from Devo Desktop. This action
					cannot be undone.
				</DialogDescription>
			</DialogHeader>

			{error && (
				<div className="rounded-lg border border-destructive/20 bg-destructive/5 px-3.5 py-2.5 text-sm text-destructive">
					{error}
				</div>
			)}

			<DialogFooter>
				<Button variant="outline" disabled={pending} onClick={onCancel}>
					Cancel
				</Button>
				<Button variant="destructive" disabled={pending} onClick={onConfirm}>
					{pending ? (
						<>
							<Loader2Icon className="size-3.5 animate-spin" />
							Deleting...
						</>
					) : (
						"Delete session"
					)}
				</Button>
			</DialogFooter>
		</>
	)
}
