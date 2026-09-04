import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue,
} from "@devo/ui/components/select"
import { Switch } from "@devo/ui/components/switch"
import { useCallback } from "react"
import { useSettings } from "../../hooks/use-settings"
import { SettingsHeader } from "./settings-header"
import { SettingsRow } from "./settings-row"
import { SettingsSection } from "./settings-section"

export function NotificationSettings() {
	const { settings, updateSettings } = useSettings()
	const notif = settings.notifications

	const updateNotif = useCallback(
		(key: string, value: unknown) => {
			updateSettings({ notifications: { [key]: value } })
		},
		[updateSettings],
	)

	const isMac =
		typeof window !== "undefined" && "devo" in window && window.devo.platform === "darwin"

	return (
		<div className="space-y-8">
			<SettingsHeader title="Notifications" />

			<SettingsSection>
				<SettingsRow
					label="Completion notifications"
					description="Set when Devo alerts you that an agent is finished"
				>
					<Select
						value={notif.completionMode}
						onValueChange={(v) => updateNotif("completionMode", v)}
						items={{
							off: "Never",
							unfocused: "Only when unfocused",
							always: "Always",
						}}
					>
						<SelectTrigger className="min-w-[180px]">
							<SelectValue />
						</SelectTrigger>
						<SelectContent>
							<SelectItem value="off">Never</SelectItem>
							<SelectItem value="unfocused">Only when unfocused</SelectItem>
							<SelectItem value="always">Always</SelectItem>
						</SelectContent>
					</Select>
				</SettingsRow>
				<SettingsRow
					label="Permission notifications"
					description="Show alerts when an agent needs approval"
				>
					<Switch
						checked={notif.permissions}
						onCheckedChange={(v) => updateNotif("permissions", v)}
					/>
				</SettingsRow>
				<SettingsRow
					label="Question notifications"
					description="Show alerts when an agent asks a question"
				>
					<Switch checked={notif.questions} onCheckedChange={(v) => updateNotif("questions", v)} />
				</SettingsRow>
				<SettingsRow
					label="Error notifications"
					description="Show alerts when an agent encounters an error"
				>
					<Switch checked={notif.errors} onCheckedChange={(v) => updateNotif("errors", v)} />
				</SettingsRow>
				{isMac && (
					<SettingsRow label="Dock badge" description="Show pending count on the dock icon">
						<Switch
							checked={notif.dockBadge}
							onCheckedChange={(v) => updateNotif("dockBadge", v)}
						/>
					</SettingsRow>
				)}
			</SettingsSection>
		</div>
	)
}
