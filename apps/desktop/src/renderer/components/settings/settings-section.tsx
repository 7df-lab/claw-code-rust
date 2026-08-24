import { type ReactNode, useId } from "react"

interface SettingsSectionProps {
	title?: string
	description?: string
	action?: ReactNode
	children: ReactNode
}

export function SettingsSection({ title, description, action, children }: SettingsSectionProps) {
	const sectionId = useId()

	return (
		<section className="space-y-3" aria-labelledby={title ? sectionId : undefined}>
			{title && (
				<div className="flex items-start justify-between gap-3 px-1">
					<div>
						<h3
							id={sectionId}
							className="text-[15px] font-normal tracking-[-0.02em] text-muted-foreground"
						>
							{title}
						</h3>
						{description && (
							<p className="mt-1 text-[13px] leading-5 text-muted-foreground/80">{description}</p>
						)}
					</div>
					{action}
				</div>
			)}
			<div className="divide-y divide-border/50 overflow-hidden rounded-[18px] border border-border/60 bg-background shadow-[0_8px_32px_rgba(0,0,0,0.05)]">
				{children}
			</div>
		</section>
	)
}
