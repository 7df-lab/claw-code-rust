import { type ReactNode, useId } from "react"

interface SettingsRowProps {
	label: string
	description?: string
	/** Optional explicit ID for the control — if not provided, one is auto-generated. */
	htmlFor?: string
	children: ReactNode
}

export function SettingsRow({ label, description, htmlFor, children }: SettingsRowProps) {
	const autoId = useId()
	const controlId = htmlFor ?? autoId

	return (
		<div className="flex items-center justify-between gap-4 px-4 py-3">
			<div className="flex min-w-0 flex-col gap-0.5">
				<label htmlFor={controlId} className="text-sm font-normal tracking-tight text-foreground">
					{label}
				</label>
				{description && (
					<span id={`${controlId}-desc`} className="text-xs leading-5 text-muted-foreground">
						{description}
					</span>
				)}
			</div>
			<div className="flex shrink-0 items-center gap-2">{children}</div>
		</div>
	)
}
