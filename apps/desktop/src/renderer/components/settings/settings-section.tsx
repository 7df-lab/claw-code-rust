import { type ReactNode, useId } from "react"
import { settingsCardClass } from "./settings-surface"

interface SettingsSectionProps {
	title?: string
	description?: string
	action?: ReactNode
	children: ReactNode
}

export function SettingsSection({ title, description, action, children }: SettingsSectionProps) {
	const sectionId = useId()

	return (
		<section className="space-y-2" aria-labelledby={title ? sectionId : undefined}>
			{title && (
				<div className="flex items-start justify-between gap-3 px-0.5">
					<div className="min-w-0">
						<h3
							id={sectionId}
							className="text-sm font-medium tracking-tight text-foreground"
						>
							{title}
						</h3>
						{description && (
							<p className="mt-0.5 text-xs leading-5 text-muted-foreground">{description}</p>
						)}
					</div>
					{action}
				</div>
			)}
			<div className={settingsCardClass}>{children}</div>
		</section>
	)
}
