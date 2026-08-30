import { cn } from "@devo/ui/lib/utils"
import { useNavigate } from "@tanstack/react-router"
import { SplitIcon } from "lucide-react"

export function ForkBoundaryDivider({
	parentName,
	sourceSessionId,
	projectSlug,
	className,
}: {
	parentName?: string
	sourceSessionId?: string
	projectSlug?: string
	className?: string
}) {
	const navigate = useNavigate()
	const displayName = parentName || "source session"
	const label = `Forked from ${displayName}`

	const handleNavigateToSource = () => {
		if (!sourceSessionId || !projectSlug) return
		navigate({
			to: "/project/$projectSlug/session/$sessionId",
			params: { projectSlug, sessionId: sourceSessionId },
		})
	}

	const canNavigate = Boolean(sourceSessionId && projectSlug)

	return (
		<div
			role="separator"
			aria-label={label}
			className={cn("flex items-center gap-3 py-2", className)}
		>
			<div className="h-px flex-1 bg-border/70" aria-hidden="true" />
			<div className="inline-flex max-w-[min(100%,24rem)] items-center gap-1.5 rounded-full border border-border/60 bg-muted/25 px-2.5 py-1 text-[11px] font-medium tracking-wide text-muted-foreground/85">
				<SplitIcon className="size-3.5 shrink-0 stroke-[1.5]" aria-hidden="true" />
				<span className="truncate">
					Forked from{" "}
					{canNavigate ? (
						<button
							type="button"
							onClick={handleNavigateToSource}
							className="font-medium text-foreground/90 underline-offset-2 transition-colors hover:text-foreground hover:underline"
						>
							{displayName}
						</button>
					) : (
						<span className="font-medium text-foreground/90">{displayName}</span>
					)}
				</span>
			</div>
			<div className="h-px flex-1 bg-border/70" aria-hidden="true" />
		</div>
	)
}
