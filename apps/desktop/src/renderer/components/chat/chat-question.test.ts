import { describe, expect, test } from "bun:test"
import { readFileSync } from "node:fs"

const questionSource = readFileSync(new URL("./chat-question.tsx", import.meta.url), "utf8")
const optionMenuSource = readFileSync(
	new URL("../../../../packages/ui/src/components/option-menu-styles.tsx", import.meta.url),
	"utf8",
)
const chromeSource = readFileSync(new URL("../../desktop-chrome.css", import.meta.url), "utf8")

describe("chat question composer", () => {
	test("uses composer chrome instead of a primary-tinted card", () => {
		expect({
			composerClass: questionSource.includes("devo-composer"),
			composerRadius: chromeSource.includes("--devo-composer-radius"),
			optionMenuHover: optionMenuSource.includes("hover:bg-muted") || questionSource.includes("hover:bg-muted"),
			selectedRow: questionSource.includes("bg-muted text-foreground"),
			checkIcon: questionSource.includes("CheckIcon"),
			roundSend: questionSource.includes("rounded-full") && questionSource.includes("ArrowUpIcon"),
			noUppercaseTracking: !questionSource.includes("uppercase tracking-wide"),
			noPrimaryTintedIcon:
				!questionSource.includes("bg-primary/10") && !questionSource.includes("ring-primary"),
			noSkipForward: !questionSource.includes("SkipForwardIcon"),
			thirteenPxCopy: questionSource.includes("text-[13px]"),
		}).toEqual({
			composerClass: true,
			composerRadius: true,
			optionMenuHover: true,
			selectedRow: true,
			checkIcon: true,
			roundSend: true,
			noUppercaseTracking: true,
			noPrimaryTintedIcon: true,
			noSkipForward: true,
			thirteenPxCopy: true,
		})
	})

	test("navigates questions from the header and reviews answers before send", () => {
		expect({
			headerChevrons:
				questionSource.includes("ChevronLeftIcon") && questionSource.includes("ChevronRightIcon"),
			navInHeader: questionSource.includes('label="Previous question"'),
			reviewPage: questionSource.includes("Your answers") && questionSource.includes("AnswerReview"),
			sendOnlyOnReview: questionSource.includes("isReview") && questionSource.includes("Send answers"),
			footerNextNotSubmit: questionSource.includes('aria-label="Next question"'),
		}).toEqual({
			headerChevrons: true,
			navInHeader: true,
			reviewPage: true,
			sendOnlyOnReview: true,
			footerNextNotSubmit: true,
		})
	})
})
