import { Skeleton } from "@devo/ui/components/skeleton"
import { cn } from "@devo/ui/lib/utils"

export function ChatTurnSkeleton({ className }: { className?: string }) {
	return (
		<div className={cn("space-y-4", className)}>
			<div className="flex justify-end">
				<Skeleton className="h-10 w-[min(72%,28rem)] rounded-2xl" />
			</div>
			<div className="space-y-2">
				<Skeleton className="h-4 w-[88%]" />
				<Skeleton className="h-4 w-[76%]" />
				<Skeleton className="h-4 w-[64%]" />
			</div>
		</div>
	)
}

export function ChatLoadingSkeleton({ className }: { className?: string }) {
	return (
		<div
			className={cn(
				"min-h-[28rem] space-y-12 animate-in fade-in duration-150",
				className,
			)}
			aria-busy="true"
			aria-label="Loading conversation"
		>
			<ChatTurnSkeleton />
			<ChatTurnSkeleton />
			<ChatTurnSkeleton />
			<ChatTurnSkeleton />
		</div>
	)
}
