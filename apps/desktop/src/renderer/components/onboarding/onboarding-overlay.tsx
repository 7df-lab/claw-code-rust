/**
 * Full-page onboarding overlay.
 *
 * Renders a multi-step first-run experience that gates the main app.
 * Uses Framer Motion for step transitions and a progress indicator at the top.
 *
 * Core flow: Welcome -> Environment Check -> Provider Setup -> Complete.
 */

import { AnimatePresence, motion } from "motion/react"
import { useCallback, useState } from "react"
import { APP_BAR_HEIGHT } from "../app-bar"
import { OnboardingProgress } from "./onboarding-progress"
import { CompleteStep } from "./steps/complete-step"
import { EnvironmentCheckStep } from "./steps/environment-check-step"
import { ProviderSetupStep } from "./steps/provider-setup-step"
import { WelcomeStep } from "./steps/welcome-step"

// ============================================================
// Types
// ============================================================

export type OnboardingStep = "welcome" | "environment" | "providers" | "complete"

interface OnboardingOverlayProps {
	onComplete: (state: {
		skippedSteps: string[]
		migrationPerformed: boolean
		migratedFrom: string[]
		devoVersion: string | null
		providersConnected: number
	}) => void
}

// ============================================================
// Constants
// ============================================================

const CORE_STEPS: OnboardingStep[] = ["welcome", "environment", "providers", "complete"]

const STEP_TRANSITION = {
	initial: { opacity: 0, y: 16 },
	animate: { opacity: 1, y: 0 },
	exit: { opacity: 0, y: -16 },
	transition: { duration: 0.25, ease: "easeOut" as const },
}

// ============================================================
// Component
// ============================================================

export function OnboardingOverlay({ onComplete }: OnboardingOverlayProps) {
	const [currentStep, setCurrentStep] = useState<OnboardingStep>("welcome")
	const [skippedSteps, setSkippedSteps] = useState<string[]>([])
	const [devoVersion, setDevoVersion] = useState<string | null>(null)
	const [providersConnected, setProvidersConnected] = useState(0)

	const displayIndex = CORE_STEPS.indexOf(currentStep)

	const goToStep = useCallback((step: OnboardingStep) => {
		setCurrentStep(step)
	}, [])

	const skipStep = useCallback((stepId: string) => {
		setSkippedSteps((prev) => [...prev, stepId])
	}, [])

	const handleWelcomeContinue = useCallback(() => {
		goToStep("environment")
	}, [goToStep])

	const handleEnvironmentComplete = useCallback(
		(version: string | null) => {
			setDevoVersion(version)
			goToStep("providers")
		},
		[goToStep],
	)

	const handleProvidersComplete = useCallback(
		(count: number) => {
			setProvidersConnected(count)
			goToStep("complete")
		},
		[goToStep],
	)

	const handleProvidersSkip = useCallback(() => {
		skipStep("providers")
		goToStep("complete")
	}, [goToStep, skipStep])

	const handleFinish = useCallback(() => {
		onComplete({
			skippedSteps,
			migrationPerformed: false,
			migratedFrom: [],
			devoVersion,
			providersConnected,
		})
	}, [onComplete, skippedSteps, devoVersion, providersConnected])

	return (
		<div
			data-slot="onboarding-overlay"
			className="fixed inset-0 z-50 flex flex-col bg-background text-foreground"
		>
			{/* Reserve space for traffic lights / app bar area */}
			<div
				className="shrink-0"
				style={{
					height: APP_BAR_HEIGHT,
					// @ts-expect-error -- vendor-prefixed CSS property
					WebkitAppRegion: "drag",
				}}
			/>

			{/* Progress indicator */}
			<div className="shrink-0 px-8 py-2">
				<OnboardingProgress
					steps={CORE_STEPS}
					currentStep={currentStep}
					currentIndex={displayIndex}
					total={CORE_STEPS.length}
				/>
			</div>

			{/* Step content with transitions */}
			<div className="relative min-h-0 flex-1 overflow-hidden">
				<AnimatePresence mode="wait">
					{currentStep === "welcome" && (
						<motion.div
							key="welcome"
							className="absolute inset-0 overflow-y-auto"
							{...STEP_TRANSITION}
						>
							<WelcomeStep onContinue={handleWelcomeContinue} />
						</motion.div>
					)}

					{currentStep === "environment" && (
						<motion.div
							key="environment"
							className="absolute inset-0 overflow-y-auto"
							{...STEP_TRANSITION}
						>
							<EnvironmentCheckStep onComplete={handleEnvironmentComplete} />
						</motion.div>
					)}

					{currentStep === "providers" && (
						<motion.div
							key="providers"
							className="absolute inset-0 overflow-y-auto"
							{...STEP_TRANSITION}
						>
							<ProviderSetupStep
								onComplete={handleProvidersComplete}
								onSkip={handleProvidersSkip}
							/>
						</motion.div>
					)}

					{currentStep === "complete" && (
						<motion.div
							key="complete"
							className="absolute inset-0 overflow-y-auto"
							{...STEP_TRANSITION}
						>
							<CompleteStep devoVersion={devoVersion} onFinish={handleFinish} />
						</motion.div>
					)}
				</AnimatePresence>
			</div>
		</div>
	)
}
