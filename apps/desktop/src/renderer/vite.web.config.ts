/**
 * Standalone Vite config for browser-mode development (no Electron).
 * Usage: bun run dev:web (or `vite --config src/renderer/vite.web.config.ts`)
 *
 * In this mode, the Devo Bun server (apps/server) must be running
 * on port 3100 to handle filesystem operations and process management.
 */

import path from "node:path"
import tailwindcss from "@tailwindcss/vite"
import react from "@vitejs/plugin-react"
import { defineConfig } from "vite"
import { protocolTypesPlugin } from "../../scripts/protocol-types"

export default defineConfig({
	root: __dirname,
	plugins: [
		protocolTypesPlugin({ desktopDir: path.resolve(__dirname, "../..") }),
		react(),
		tailwindcss(),
	],
	resolve: {
		alias: {
			"@": __dirname,
			"@devo/ui": path.resolve(__dirname, "../../packages/ui/src"),
			"@devo-ai/sdk/v2/client": path.resolve(
				__dirname,
				"../../packages/devo-ai-sdk/src/v2/client.ts",
			),
			"@devo-ai/sdk/v2/protocol-validation": path.resolve(
				__dirname,
				"../../packages/devo-ai-sdk/src/v2/protocol-validation.ts",
			),
		},
	},
	worker: {
		format: "es",
	},
	clearScreen: false,
	server: {
		port: 1420,
		strictPort: true,
	},
})
