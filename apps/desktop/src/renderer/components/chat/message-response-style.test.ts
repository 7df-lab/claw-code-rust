import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const messageSource = readFileSync(
	new URL("../../../../packages/ui/src/components/ai-elements/message.tsx", import.meta.url),
	"utf8",
)
const uiStylesSource = readFileSync(
	new URL("../../../../packages/ui/src/styles/globals.css", import.meta.url),
	"utf8",
)
const rendererCssSource = readFileSync(new URL("../../index.css", import.meta.url), "utf8")

describe("MessageResponse markdown surfaces", () => {
	test("wires Inter Variable and IBM Plex Mono into theme font tokens", () => {
		expect({
			sansInter: uiStylesSource.includes('"Inter Variable"'),
			monoPlex: uiStylesSource.includes('"IBM Plex Mono"'),
			rendererImportsInter: rendererCssSource.includes("@fontsource-variable/inter"),
			rendererImportsPlex: rendererCssSource.includes("@fontsource/ibm-plex-mono"),
			markdownReadingSurface: rendererCssSource.includes(
				"Transcript markdown — European minimal reading surface",
			),
		}).toEqual({
			sansInter: true,
			monoPlex: true,
			rendererImportsInter: true,
			rendererImportsPlex: true,
			markdownReadingSurface: true,
		})
	})

	test("uses desktop dark theme surfaces for streamdown markdown cells", () => {
		expect({
			responseClass: messageSource.includes("devo-message-response"),
			codeBlockSurface: rendererCssSource.includes('[data-streamdown="code-block"]'),
			codeBlockBodySurface: rendererCssSource.includes('[data-streamdown="code-block-body"]'),
			tableHeaderSurface: rendererCssSource.includes('[data-streamdown="table-header"]'),
		}).toEqual({
			responseClass: true,
			codeBlockSurface: true,
			codeBlockBodySurface: true,
			tableHeaderSurface: true,
		})
	})

	test("keeps transcript markdown size aligned with chrome and strong weight visible for CJK", () => {
		expect({
			markdownBodySize: rendererCssSource.includes("font-size: 0.875rem;"),
			strongWeight: rendererCssSource.includes(
				".devo-message-response [data-streamdown=\"strong\"]",
			),
			strongUsesSemibold: /\[data-streamdown="strong"\]\s*\{[^}]*font-weight:\s*600/.test(
				rendererCssSource,
			),
			cjkWeightNote: rendererCssSource.includes("CJK fallbacks"),
		}).toEqual({
			markdownBodySize: true,
			strongWeight: true,
			strongUsesSemibold: true,
			cjkWeightNote: true,
		})
	})

	test("keeps transcript markdown headings visually compact", () => {
		expect({
			requirementComment: messageSource.includes(
				"transcript Markdown headings should look like bold body text",
			),
			headingComponents: messageSource.includes("const transcriptMarkdownComponents"),
			headingStyle: messageSource.includes(
				"mt-3 mb-1 border-0 p-0 text-[14px] font-semibold leading-snug tracking-normal text-foreground first:mt-0",
			),
			markdownRulesHidden: messageSource.includes("hr: TranscriptMarkdownRule"),
			markdownRulesRequirementComment: messageSource.includes(
				"Horizontal rules (--- / ***) are hidden",
			),
			markdownRuleReturnsNull: messageSource.includes("function TranscriptMarkdownRule"),
		}).toEqual({
			requirementComment: true,
			headingComponents: true,
			headingStyle: true,
			markdownRulesHidden: true,
			markdownRulesRequirementComment: true,
			markdownRuleReturnsNull: true,
		})
	})

	test("keeps streamdown code block actions in the language header row", () => {
		expect({
			headerPadding: rendererCssSource.includes('[data-streamdown="code-block-header"]'),
			actionsSiblingSelector: rendererCssSource.includes(
				'> div:has(> [data-streamdown="code-block-actions"])',
			),
			actionsAbsolute: rendererCssSource.includes("position: absolute;"),
			actionsStillClickable: rendererCssSource.includes("pointer-events: auto;"),
		}).toEqual({
			headerPadding: true,
			actionsSiblingSelector: true,
			actionsAbsolute: true,
			actionsStillClickable: true,
		})
	})

	test("removes fullscreen from regular markdown table controls only", () => {
		expect({
			controlsConfig: messageSource.includes("const transcriptMarkdownControls"),
			tableFullscreenDisabled: messageSource.includes("fullscreen: false"),
			controlsPassedToStreamdown: messageSource.includes("controls={transcriptMarkdownControls}"),
			tableCopyNotDisabled: !messageSource.includes("copy: false"),
			tableDownloadNotDisabled: !messageSource.includes("download: false"),
		}).toEqual({
			controlsConfig: true,
			tableFullscreenDisabled: true,
			controlsPassedToStreamdown: true,
			tableCopyNotDisabled: true,
			tableDownloadNotDisabled: true,
		})
	})

	test("includes streamdown sources so code highlighting classes are generated", () => {
		expect({
			streamdownSource: uiStylesSource.includes('@source "../../../../node_modules/streamdown/dist/*.js";'),
			codePluginSource: uiStylesSource.includes(
				'@source "../../../../node_modules/@streamdown/code/dist/*.js";',
			),
			cjkPluginSource: uiStylesSource.includes(
				'@source "../../../../node_modules/@streamdown/cjk/dist/*.js";',
			),
			mathPluginSource: uiStylesSource.includes(
				'@source "../../../../node_modules/@streamdown/math/dist/*.js";',
			),
			mermaidPluginSource: uiStylesSource.includes(
				'@source "../../../../node_modules/@streamdown/mermaid/dist/*.js";',
			),
		}).toEqual({
			streamdownSource: true,
			codePluginSource: true,
			cjkPluginSource: true,
			mathPluginSource: true,
			mermaidPluginSource: true,
		})
	})
})
