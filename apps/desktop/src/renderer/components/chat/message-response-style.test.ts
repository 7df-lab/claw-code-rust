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
	test("wires Inter Variable, Noto Sans SC Variable, and IBM Plex Mono into theme font tokens", () => {
		expect({
			sansInter: uiStylesSource.includes('"Inter Variable"'),
			sansNoto: uiStylesSource.includes('"Noto Sans SC Variable"'),
			monoPlex: uiStylesSource.includes('"IBM Plex Mono"'),
			rendererImportsInter: rendererCssSource.includes("@fontsource-variable/inter"),
			rendererImportsNoto: rendererCssSource.includes("@fontsource-variable/noto-sans-sc"),
			rendererImportsPlex: rendererCssSource.includes("@fontsource/ibm-plex-mono"),
			markdownReadingSurface: rendererCssSource.includes(
				"Transcript markdown — European minimal reading surface",
			),
		}).toEqual({
			sansInter: true,
			sansNoto: true,
			monoPlex: true,
			rendererImportsInter: true,
			rendererImportsNoto: true,
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

	test("keeps transcript markdown size aligned with chrome and mid-weight strong for CJK", () => {
		expect({
			markdownBodySize: rendererCssSource.includes("font-size: 0.875rem;"),
			strongWeight: rendererCssSource.includes(
				'.devo-message-response [data-streamdown="strong"]',
			),
			strongUsesMidWeight: /\[data-streamdown="strong"\]\s*\{[^}]*font-weight:\s*530/.test(
				rendererCssSource,
			),
			strongDisablesSynthesis: /\[data-streamdown="strong"\]\s*\{[^}]*font-synthesis:\s*none/.test(
				rendererCssSource,
			),
			cjkNotoNote: rendererCssSource.includes("Noto Sans SC Variable"),
		}).toEqual({
			markdownBodySize: true,
			strongWeight: true,
			strongUsesMidWeight: true,
			strongDisablesSynthesis: true,
			cjkNotoNote: true,
		})
	})

	test("keeps transcript markdown headings visually compact", () => {
		expect({
			requirementComment: messageSource.includes(
				"transcript Markdown headings should look like bold body text",
			),
			headingComponents: messageSource.includes("const transcriptMarkdownComponents"),
			headingStyle: messageSource.includes(
				"mt-3 mb-1 border-0 p-0 text-[14px] font-[530] leading-snug tracking-normal text-foreground first:mt-0",
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
			compactHeaderHeight: rendererCssSource.includes("height: 1.5rem;"),
			compactBodyPadding: rendererCssSource.includes("padding: 0.45rem 0.7rem;"),
			actionsSiblingSelector: rendererCssSource.includes(
				'> div:has(> [data-streamdown="code-block-actions"])',
			),
			actionsAbsolute: rendererCssSource.includes("position: absolute;"),
			actionsStillClickable: rendererCssSource.includes("pointer-events: auto;"),
		}).toEqual({
			headerPadding: true,
			compactHeaderHeight: true,
			compactBodyPadding: true,
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
			tableCopyNotDisabled: !/table:\s*\{[^}]*copy:\s*false/.test(messageSource),
			codeDownloadDisabled: /code:\s*\{[^}]*download:\s*false/.test(messageSource),
		}).toEqual({
			controlsConfig: true,
			tableFullscreenDisabled: true,
			controlsPassedToStreamdown: true,
			tableCopyNotDisabled: true,
			codeDownloadDisabled: true,
		})
	})

	test("uses a lighter Streamdown plugin set while streaming", () => {
		expect({
			streamingProp: messageSource.includes("streaming?: boolean"),
			streamingPlugins: messageSource.includes("streamdownPluginsStreaming"),
			disablesAnimatedWhileStreaming: messageSource.includes(
				"animated={streaming ? false : animated}",
			),
			streamingClass: messageSource.includes("devo-message-response--streaming"),
			streamFadeCss: rendererCssSource.includes("devo-stream-surface-in"),
			streamTailCss: rendererCssSource.includes("devo-stream-tail-in"),
		}).toEqual({
			streamingProp: true,
			streamingPlugins: true,
			disablesAnimatedWhileStreaming: true,
			streamingClass: true,
			streamFadeCss: true,
			streamTailCss: true,
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
