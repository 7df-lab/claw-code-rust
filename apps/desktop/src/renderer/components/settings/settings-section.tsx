import { type ReactNode, useId } from "react"

interface SettingsSectionProps {
	title?: string
	description?: string
	children: ReactNode
}

export function SettingsSection({ title, description, children }: SettingsSectionProps) {
	const sectionId = useId()

	return (
		<section className="space-y-3" aria-labelledby={title ? sectionId : undefined}>
			{title && (
				<div>
					<h3 id={sectionId} className="text-[13px] font-medium tracking-wide text-muted-foreground">
						{title}
					</h3>
					{description && <p className="mt-1 text-[13px] leading-5 text-muted-foreground">{description}</p>}
				</div>
			)}
			<div className="divide-y divide-border/70 overflow-hidden rounded-xl border border-border/70">{children}</div>
		</section>
	)
}
