import { execFileSync } from "node:child_process"
import { copyFileSync, existsSync, mkdirSync, renameSync, unlinkSync, utimesSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

const appName = "Devo"
const scriptDir = dirname(fileURLToPath(import.meta.url))
const desktopDir = dirname(scriptDir)

function brandDarwin() {
	const bundleIdentifier = "com.devo.desktop.dev"
	const electronAppPath = join(desktopDir, "node_modules/electron/dist/Electron.app")
	const plistPath = join(electronAppPath, "Contents/Info.plist")
	const iconSourcePath = join(desktopDir, "resources/icon.icns")
	const iconTargetPath = join(electronAppPath, "Contents/Resources/electron.icns")

	if (!existsSync(plistPath) || !existsSync(iconSourcePath)) {
		return
	}

	function setPlistString(key, value) {
		try {
			execFileSync("/usr/libexec/PlistBuddy", ["-c", `Set :${key} ${value}`, plistPath], {
				stdio: "ignore",
			})
		} catch {
			execFileSync("/usr/libexec/PlistBuddy", ["-c", `Add :${key} string ${value}`, plistPath], {
				stdio: "ignore",
			})
		}
	}

	mkdirSync(dirname(iconTargetPath), { recursive: true })
	copyFileSync(iconSourcePath, iconTargetPath)
	setPlistString("CFBundleName", appName)
	setPlistString("CFBundleDisplayName", appName)
	setPlistString("CFBundleIdentifier", bundleIdentifier)
	setPlistString("CFBundleIconFile", "electron.icns")

	const now = new Date()
	utimesSync(electronAppPath, now, now)

	console.log(`Branded Electron dev app as ${appName}`)
}

async function brandWin32() {
	const electronExePath = join(desktopDir, "node_modules/electron/dist/electron.exe")
	const iconSourcePath = join(desktopDir, "resources/icon.ico")
	const brandedExePath = `${electronExePath}.branded`

	if (!existsSync(electronExePath) || !existsSync(iconSourcePath)) {
		return
	}

	const { rcedit } = await import("rcedit")
	copyFileSync(electronExePath, brandedExePath)
	try {
		await rcedit(brandedExePath, {
			icon: iconSourcePath,
			"version-string": {
				CompanyName: "Devo",
				FileDescription: appName,
				InternalName: appName,
				OriginalFilename: "electron.exe",
				ProductName: appName,
			},
		})
		try {
			renameSync(brandedExePath, electronExePath)
		} catch {
			console.warn(
				"Could not replace electron.exe while it is running. Close the Desktop client, then run `bun run brand:electron-dev` again.",
			)
			return
		}
	} finally {
		if (existsSync(brandedExePath)) {
			unlinkSync(brandedExePath)
		}
	}

	console.log(`Branded Electron.exe for Windows as ${appName}`)
}

if (process.platform === "darwin") {
	brandDarwin()
} else if (process.platform === "win32") {
	try {
		await brandWin32()
	} catch (error) {
		const message = error instanceof Error ? error.message : String(error)
		console.warn(`Skipping Windows Electron branding: ${message}`)
	}
}
