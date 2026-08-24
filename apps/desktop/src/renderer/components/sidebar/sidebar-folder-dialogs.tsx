import { Button } from "@devo/ui/components/button"
import { Dialog, DialogContent } from "@devo/ui/components/dialog"
import { Input } from "@devo/ui/components/input"
import { AlertTriangleIcon, FolderPlusIcon, Loader2Icon, TrashIcon } from "lucide-react"
import type { SidebarProject } from "../../lib/types"

export function FolderRemoveDialog({
	project,
	open,
	pending,
	error,
	onOpenChange,
	onConfirm,
}: {
	project: SidebarProject | null
	open: boolean
	pending: boolean
	error: string | null
	onOpenChange: (open: boolean) => void
	onConfirm: () => void
}) {
	return (
		<Dialog open={open} onOpenChange={(isOpen) => !pending && onOpenChange(isOpen)}>
			<DialogContent showCloseButton={false} className="sm:max-w-md">
				<FolderRemoveDialogBody
					project={project}
					pending={pending}
					error={error}
					onCancel={() => onOpenChange(false)}
					onConfirm={onConfirm}
				/>
			</DialogContent>
		</Dialog>
	)
}

export function FolderRemoveDialogBody({
	project,
	pending,
	error,
	onCancel,
	onConfirm,
}: {
	project: SidebarProject | null
	pending: boolean
	error: string | null
	onCancel: () => void
	onConfirm: () => void
}) {
	return (
		<>
			<div className="flex flex-col gap-2">
				<h2 className="flex items-center gap-2 text-lg font-semibold">
					<AlertTriangleIcon className="size-5 text-destructive" />
					Remove folder from Devo Desktop
				</h2>
				<p className="text-sm text-muted-foreground">
					Remove{" "}
					<span className="font-medium text-foreground">{project?.name || "this folder"}</span>{" "}
					from Devo Desktop? This permanently deletes all sessions in this folder and cannot
					be undone. The folder on disk will not be deleted.
				</p>
			</div>
			{error && (
				<div className="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
					{error}
				</div>
			)}
			<div className="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
				<Button variant="outline" disabled={pending} onClick={onCancel}>
					Cancel
				</Button>
				<Button variant="destructive" disabled={pending} onClick={onConfirm}>
					{pending ? (
						<Loader2Icon className="size-3.5 animate-spin" />
					) : (
						<TrashIcon className="size-3.5" />
					)}
					{pending ? "Removing" : "Remove"}
				</Button>
			</div>
		</>
	)
}

export function MissingFolderDialog({
	project,
	open,
	pending,
	error,
	onOpenChange,
	onConfirmRemove,
}: {
	project: SidebarProject | null
	open: boolean
	pending: boolean
	error: string | null
	onOpenChange: (open: boolean) => void
	onConfirmRemove: () => void
}) {
	return (
		<Dialog open={open} onOpenChange={(isOpen) => !pending && onOpenChange(isOpen)}>
			<DialogContent showCloseButton={false} className="sm:max-w-md">
				<MissingFolderDialogBody
					project={project}
					pending={pending}
					error={error}
					onCancel={() => onOpenChange(false)}
					onConfirmRemove={onConfirmRemove}
				/>
			</DialogContent>
		</Dialog>
	)
}

export function MissingFolderDialogBody({
	project,
	pending,
	error,
	onCancel,
	onConfirmRemove,
}: {
	project: SidebarProject | null
	pending: boolean
	error: string | null
	onCancel: () => void
	onConfirmRemove: () => void
}) {
	return (
		<>
			<div className="flex flex-col gap-2">
				<h2 className="flex items-center gap-2 text-lg font-semibold">
					<AlertTriangleIcon className="size-5 text-destructive" />
					Folder no longer exists
				</h2>
				<p className="text-sm text-muted-foreground">
					<span className="font-medium text-foreground">{project?.name || "This folder"}</span>{" "}
					cannot be found on disk. Remove it from Devo Desktop? This permanently deletes all
					sessions in this folder and cannot be undone.
				</p>
			</div>
			{error && (
				<div className="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
					{error}
				</div>
			)}
			<div className="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
				<Button variant="outline" disabled={pending} onClick={onCancel}>
					Cancel
				</Button>
				<Button variant="destructive" disabled={pending} onClick={onConfirmRemove}>
					{pending ? (
						<Loader2Icon className="size-3.5 animate-spin" />
					) : (
						<TrashIcon className="size-3.5" />
					)}
					{pending ? "Removing" : "Remove"}
				</Button>
			</div>
		</>
	)
}

export function CreateFolderDialog({
	open,
	parentDirectory,
	name,
	pending,
	error,
	onOpenChange,
	onPickParent,
	onParentDirectoryChange,
	onNameChange,
	onSubmit,
}: {
	open: boolean
	parentDirectory: string
	name: string
	pending: boolean
	error: string | null
	onOpenChange: (open: boolean) => void
	onPickParent: () => void
	onParentDirectoryChange: (directory: string) => void
	onNameChange: (name: string) => void
	onSubmit: () => void
}) {
	return (
		<Dialog open={open} onOpenChange={(isOpen) => !pending && onOpenChange(isOpen)}>
			<DialogContent showCloseButton={false} className="sm:max-w-lg">
				<CreateFolderDialogBody
					parentDirectory={parentDirectory}
					name={name}
					pending={pending}
					error={error}
					onCancel={() => onOpenChange(false)}
					onPickParent={onPickParent}
					onParentDirectoryChange={onParentDirectoryChange}
					onNameChange={onNameChange}
					onSubmit={onSubmit}
				/>
			</DialogContent>
		</Dialog>
	)
}

export function CreateFolderDialogBody({
	parentDirectory,
	name,
	pending,
	error,
	onCancel,
	onPickParent,
	onParentDirectoryChange,
	onNameChange,
	onSubmit,
}: {
	parentDirectory: string
	name: string
	pending: boolean
	error: string | null
	onCancel: () => void
	onPickParent: () => void
	onParentDirectoryChange: (directory: string) => void
	onNameChange: (name: string) => void
	onSubmit: () => void
}) {
	return (
		<>
			<div className="flex flex-col gap-2">
				<h2 className="flex items-center gap-2 text-lg font-semibold">
					<FolderPlusIcon className="size-5" />
					Create folder
				</h2>
			</div>
			<form
				className="flex flex-col gap-4"
				onSubmit={(event) => {
					event.preventDefault()
					onSubmit()
				}}
			>
				<div className="flex flex-col gap-2">
					<label className="text-sm font-medium" htmlFor="desktop-folder-parent">
						Parent directory
					</label>
					<div className="flex gap-2">
						<Input
							id="desktop-folder-parent"
							value={parentDirectory}
							onChange={(event) => onParentDirectoryChange(event.target.value)}
							placeholder="Choose a parent directory"
						/>
						<Button type="button" variant="outline" disabled={pending} onClick={onPickParent}>
							Choose
						</Button>
					</div>
				</div>
				<div className="flex flex-col gap-2">
					<label className="text-sm font-medium" htmlFor="desktop-folder-name">
						Folder name
					</label>
					<Input
						id="desktop-folder-name"
						value={name}
						onChange={(event) => onNameChange(event.target.value)}
						placeholder="New folder"
					/>
				</div>
				{error && (
					<div className="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
						{error}
					</div>
				)}
				<div className="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
					<Button type="button" variant="outline" disabled={pending} onClick={onCancel}>
						Cancel
					</Button>
					<Button type="submit" disabled={pending}>
						{pending ? (
							<Loader2Icon className="size-3.5 animate-spin" />
						) : (
							<FolderPlusIcon className="size-3.5" />
						)}
						{pending ? "Creating" : "Create"}
					</Button>
				</div>
			</form>
		</>
	)
}
