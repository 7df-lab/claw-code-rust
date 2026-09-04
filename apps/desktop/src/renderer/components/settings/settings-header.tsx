import type { ReactNode } from "react"

interface SettingsHeaderProps {
	title: string
	description?: ReactNode
	action?: ReactNode
}

export function SettingsHeader({ title, description, action }: SettingsHeaderProps) {
	return (
		<div className="flex items-start justify-between gap-4">
			<div className="min-w-0">
				<h2 className="select-none text-2xl font-medium leading-tight tracking-tight text-foreground">
					{title}
				</h2>
				{description ? (
					<p className="mt-1.5 max-w-xl text-sm leading-6 text-muted-foreground">{description}</p>
				) : null}
			</div>
			{action ? <div className="mt-0.5 shrink-0">{action}</div> : null}
		</div>
	)
}
