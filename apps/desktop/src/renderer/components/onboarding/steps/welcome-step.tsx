/**
 * Onboarding Step 1: Welcome.
 *
 * Brief introduction to Devo and what the setup will cover.
 */

import { Button } from "@devo/ui/components/button"
import { ArrowRightIcon } from "lucide-react"

interface WelcomeStepProps {
	onContinue: () => void
}

export function WelcomeStep({ onContinue }: WelcomeStepProps) {
	return (
		<div className="flex h-full flex-col items-center justify-center px-6">
			<div className="w-full max-w-md space-y-8 text-center">
				{/* Brand */}
				<div className="space-y-5">
					<div className="flex justify-center">
						<div
							data-slot="welcome-brand-mark"
							className="flex size-16 items-center justify-center rounded-full bg-primary/10 text-foreground"
							role="img"
							aria-label="Devo"
						>
							<svg
								viewBox="0 0 128 128"
								fill="none"
								xmlns="http://www.w3.org/2000/svg"
								className="size-9"
								aria-hidden="true"
							>
								<g
									fill="none"
									stroke="currentColor"
									strokeLinecap="round"
									strokeLinejoin="round"
									strokeWidth="6"
								>
									<path d="M64 17.5C78.5 25.2 86.2 40.8 82.6 56.8C67.4 57.6 54.3 49.9 48.8 36.1C51.9 26.7 57.2 20.4 64 17.5Z" />
									<path
										d="M64 17.5C78.5 25.2 86.2 40.8 82.6 56.8C67.4 57.6 54.3 49.9 48.8 36.1C51.9 26.7 57.2 20.4 64 17.5Z"
										transform="rotate(60 64 64)"
									/>
									<path
										d="M64 17.5C78.5 25.2 86.2 40.8 82.6 56.8C67.4 57.6 54.3 49.9 48.8 36.1C51.9 26.7 57.2 20.4 64 17.5Z"
										transform="rotate(120 64 64)"
									/>
									<path
										d="M64 17.5C78.5 25.2 86.2 40.8 82.6 56.8C67.4 57.6 54.3 49.9 48.8 36.1C51.9 26.7 57.2 20.4 64 17.5Z"
										transform="rotate(180 64 64)"
									/>
									<path
										d="M64 17.5C78.5 25.2 86.2 40.8 82.6 56.8C67.4 57.6 54.3 49.9 48.8 36.1C51.9 26.7 57.2 20.4 64 17.5Z"
										transform="rotate(240 64 64)"
									/>
									<path
										d="M64 17.5C78.5 25.2 86.2 40.8 82.6 56.8C67.4 57.6 54.3 49.9 48.8 36.1C51.9 26.7 57.2 20.4 64 17.5Z"
										transform="rotate(300 64 64)"
									/>
								</g>
								<circle cx="64" cy="64" r="9.5" fill="#60A5FA" />
							</svg>
						</div>
					</div>
					<h2 className="text-[28px] font-medium tracking-tight text-foreground">Devo</h2>
				</div>

				{/* Description */}
				<div className="space-y-3">
					<p className="text-[15px] leading-6 text-muted-foreground">Your desktop companion for Devo.</p>
					<p className="text-sm leading-relaxed text-muted-foreground/70">
						Devo gives you a native experience for managing Devo sessions across all your
						projects, with real-time streaming, native notifications, and multi-session support.
					</p>
				</div>

				{/* CTA */}
				<div className="space-y-3">
					<Button size="lg" onClick={onContinue} className="gap-2">
						Get Started
						<ArrowRightIcon aria-hidden="true" className="size-4" />
					</Button>
					<p className="text-xs text-muted-foreground/50">This takes less than a minute.</p>
				</div>
			</div>
		</div>
	)
}
