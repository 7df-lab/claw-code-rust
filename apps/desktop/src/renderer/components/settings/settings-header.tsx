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
				<h2 className="select-none text-[32px] font-normal leading-tight tracking-[-0.03em] text-foreground">
					{title}
				</h2>
				{description ? (
					<p className="mt-2 max-w-xl text-[15px] leading-6 text-muted-foreground">{description}</p>
				) : null}
			</div>
			{action ? <div className="mt-1.5 shrink-0">{action}</div> : null}
		</div>
	)
}
