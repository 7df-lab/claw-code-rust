import { describe, expect, mock, test } from "bun:test"
import type { ProviderVendor } from "@devo-ai/sdk/v2/client"
import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import type React from "react"
import { renderToStaticMarkup } from "react-dom/server"
import type { ProviderVendorFormValues } from "./provider-vendor-dialog"

mock.module("@devo/ui/components/dialog", () => ({
	Dialog: ({ children }: { children: React.ReactNode }) => <div data-slot="dialog">{children}</div>,
	DialogContent: ({ children, ...props }: React.ComponentProps<"div">) => (
		<div data-slot="dialog-content" {...props}>
			{children}
		</div>
	),
	DialogDescription: ({ children, ...props }: React.ComponentProps<"p">) => (
		<p data-slot="dialog-description" {...props}>
			{children}
		</p>
	),
	DialogFooter: ({ children, ...props }: React.ComponentProps<"div">) => (
		<div data-slot="dialog-footer" {...props}>
			{children}
		</div>
	),
	DialogHeader: ({ children, ...props }: React.ComponentProps<"div">) => (
		<div data-slot="dialog-header" {...props}>
			{children}
		</div>
	),
	DialogTitle: ({ children, ...props }: React.ComponentProps<"h2">) => (
		<h2 data-slot="dialog-title" {...props}>
			{children}
		</h2>
	),
}))

const { ProviderSettingsView } = await import("./provider-settings")
const { buildProviderUpsertParams, ProviderVendorDialog, saveProviderVendor } = await import(
	"./provider-vendor-dialog"
)

const providerVendor = {
	name: "openai",
	base_url: "https://api.openai.com/v1",
	credential: "openai_api_key",
	headers: null,
	wire_apis: ["openai_chat_completions"],
	enabled: true,
} satisfies ProviderVendor

const formValues = {
	providerName: "openai",
	baseUrl: "https://api.openai.com/v1",
	wireApi: "openai_chat_completions",
	enabled: true,
	headers: "",
	apiKey: "secret",
	bindingId: "openai-gpt-4o",
	modelSlug: "gpt-4o",
	requestModel: "gpt-4o",
	displayName: "GPT-4o",
	defaultReasoningEffort: "",
	makeDefault: true,
} satisfies ProviderVendorFormValues

describe("ProviderSettings", () => {
	test("empty provider list shows add provider action without catalog count", () => {
		const markup = renderToStaticMarkup(
			<ProviderSettingsView
				providerVendors={[]}
				loading={false}
				error={null}
				onReload={() => {}}
			/>,
		)

		expect({
			hasHeading: markup.includes(">Providers</h2>"),
			hasAddProvider: markup.includes("Add Provider"),
			hasEmptyState: markup.includes("No providers configured"),
			hasOldCatalogCount: markup.includes("Browse all 0 providers"),
		}).toEqual({
			hasHeading: true,
			hasAddProvider: true,
			hasEmptyState: true,
			hasOldCatalogCount: false,
		})
	})

	test("configured provider row shows server vendor fields", () => {
		const markup = renderToStaticMarkup(
			<ProviderSettingsView
				providerVendors={[providerVendor]}
				loading={false}
				error={null}
				onReload={() => {}}
			/>,
		)

		expect({
			hasProviderName: markup.includes("openai"),
			hasBaseUrl: markup.includes("https://api.openai.com/v1"),
			hasWireApi: markup.includes("openai_chat_completions"),
			hasEnabled: markup.includes("Enabled"),
			hasEdit: markup.includes(">Edit</button>"),
		}).toEqual({
			hasProviderName: true,
			hasBaseUrl: true,
			hasWireApi: true,
			hasEnabled: true,
			hasEdit: true,
		})
	})

	test("blank API key on edit preserves credential and omits api_key", () => {
		const params = buildProviderUpsertParams(
			{
				...formValues,
				apiKey: "   ",
				makeDefault: false,
			},
			providerVendor,
		)

		expect(params).toEqual({
			provider_vendor: {
				name: "openai",
				base_url: "https://api.openai.com/v1",
				credential: "openai_api_key",
				headers: null,
				wire_apis: ["openai_chat_completions"],
				enabled: true,
			},
			model_binding: {
				binding_id: "openai-gpt-4o",
				model_slug: "gpt-4o",
				provider: "openai",
				request_model: "gpt-4o",
				display_name: "GPT-4o",
				invocation_method: "openai_chat_completions",
				default_reasoning_effort: null,
				enabled: true,
			},
			default_model_binding: null,
		})
	})

	test("save validates before upsert", async () => {
		const calls: unknown[] = []
		const params = buildProviderUpsertParams(formValues, null)
		const client = {
			provider: {
				validate: async (input: unknown) => {
					calls.push({ kind: "validate", input })
					return { data: { reply_preview: "OK" } }
				},
				upsert: async (input: unknown) => {
					calls.push({ kind: "upsert", input })
					return { data: { provider_vendor: providerVendor } }
				},
			},
		}

		await saveProviderVendor(client, params)

		expect(calls).toEqual([
			{
				kind: "validate",
				input: {
					provider_vendor: params.provider_vendor,
					model_binding: params.model_binding,
					api_key: params.api_key,
				},
			},
			{ kind: "upsert", input: params },
		])
	})

	test("failed validation does not upsert", async () => {
		const calls: string[] = []
		const params = buildProviderUpsertParams(formValues, null)
		const client = {
			provider: {
				validate: async () => {
					calls.push("validate")
					throw new Error("bad key")
				},
				upsert: async () => {
					calls.push("upsert")
					return { data: { provider_vendor: providerVendor } }
				},
			},
		}

		await expect(saveProviderVendor(client, params)).rejects.toThrow("bad key")
		expect(calls).toEqual(["validate"])
	})

	test("cancelled validation does not upsert", async () => {
		const calls: string[] = []
		const params = buildProviderUpsertParams(formValues, null)
		const client = {
			provider: {
				validate: async () => {
					calls.push("validate")
					return { data: { reply_preview: "OK" } }
				},
				upsert: async () => {
					calls.push("upsert")
					return { data: { provider_vendor: providerVendor } }
				},
			},
		}

		const cancelled = true
		await saveProviderVendor(client, params, () => !cancelled)
		expect(calls).toEqual([])
	})

	test("provider dialog scrolls form body while keeping footer actions outside", () => {
		const queryClient = new QueryClient()
		const markup = renderToStaticMarkup(
			<QueryClientProvider client={queryClient}>
				<ProviderVendorDialog
					providerVendor={null}
					open
					onOpenChange={() => {}}
					onSaved={() => {}}
				/>
			</QueryClientProvider>,
		)

		const scrollBodyIndex = markup.indexOf('data-testid="provider-dialog-scroll-body"')
		const footerIndex = markup.indexOf('data-testid="provider-dialog-footer"')
		const saveButtonIndex = markup.indexOf("Validate &amp; Save")

		expect({
			hasScrollBody: scrollBodyIndex >= 0,
			hasFooter: footerIndex >= 0,
			footerAfterScrollBody: footerIndex > scrollBodyIndex,
			saveButtonInFooter: footerIndex >= 0 && saveButtonIndex > footerIndex,
		}).toEqual({
			hasScrollBody: true,
			hasFooter: true,
			footerAfterScrollBody: true,
			saveButtonInFooter: true,
		})
	})
})
