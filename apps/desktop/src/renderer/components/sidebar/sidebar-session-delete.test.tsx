import { describe, expect, mock, test } from "bun:test"
import type React from "react"
import { renderToStaticMarkup } from "react-dom/server"

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

const {
	SessionDeleteDialog,
	SessionDeleteDialogBody,
	deleteSessionNavigationTarget,
} = await import("./sidebar-session-delete")

describe("session delete confirmation", () => {
	test("renders an irreversible delete confirmation", () => {
		const markup = renderToStaticMarkup(
			<SessionDeleteDialogBody
				pending={false}
				error={null}
				onCancel={() => {}}
				onConfirm={() => {}}
			/>,
		)

		expect({
			hasTitle: markup.includes("Delete session"),
			hasIrreversibleCopy: markup.includes("cannot be undone"),
			hasDeleteAction: markup.includes("Delete session"),
		}).toEqual({
			hasTitle: true,
			hasIrreversibleCopy: true,
			hasDeleteAction: true,
		})
	})

	test("shows pending and error states", () => {
		const markup = renderToStaticMarkup(
			<SessionDeleteDialogBody
				pending
				error="Failed to delete session"
				onCancel={() => {}}
				onConfirm={() => {}}
			/>,
		)

		expect({
			hasPendingLabel: markup.includes("Deleting..."),
			hasError: markup.includes("Failed to delete session"),
		}).toEqual({
			hasPendingLabel: true,
			hasError: true,
		})
	})

	test("wraps the body in a dialog shell", () => {
		const markup = renderToStaticMarkup(
			<SessionDeleteDialog
				open
				pending={false}
				error={null}
				onOpenChange={() => {}}
				onConfirm={() => {}}
			/>,
		)

		expect(markup.includes("data-slot=\"dialog-content\"")).toBe(true)
	})

	test("navigates from a deleted active session to the same project new-chat route", () => {
		expect(
			deleteSessionNavigationTarget({
				deletedSessionId: "session-1",
				currentSessionId: "session-1",
				projectSlug: "devo-123",
			}),
		).toEqual({
			to: "/project/$projectSlug",
			params: { projectSlug: "devo-123" },
		})
	})

	test("does not navigate when a background session is deleted", () => {
		expect(
			deleteSessionNavigationTarget({
				deletedSessionId: "session-2",
				currentSessionId: "session-1",
				projectSlug: "devo-123",
			}),
		).toEqual(null)
	})
})
