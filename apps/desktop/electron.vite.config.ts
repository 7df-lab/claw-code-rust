import fs from "node:fs"
import path from "node:path"
import tailwindcss from "@tailwindcss/vite"
import react from "@vitejs/plugin-react"
import { defineConfig, externalizeDepsPlugin } from "electron-vite"
import type { Plugin } from "vite"
import { protocolTypesPlugin } from "./scripts/protocol-types"

const sdkClientAlias = path.resolve(__dirname, "packages/devo-ai-sdk/src/v2/client.ts")
const sdkProtocolValidationAlias = path.resolve(
	__dirname,
	"packages/devo-ai-sdk/src/v2/protocol-validation.ts",
)

/**
 * Copies the drizzle migrations directory into the main process output.
 *
 * viteStaticCopy does not reliably fire during electron-vite's dev rebuilds,
 * so we use a plain Rollup writeBundle hook instead.
 */
function copyDrizzleMigrations(): Plugin {
	const src = path.resolve(__dirname, "drizzle")
	return {
		name: "copy-drizzle-migrations",
		writeBundle(options) {
			const dest = path.join(options.dir!, "drizzle")
			if (fs.existsSync(src)) {
				fs.cpSync(src, dest, { recursive: true })
			}
		},
	}
}

export default defineConfig({
	main: {
		plugins: [
			protocolTypesPlugin({ desktopDir: __dirname }),
			externalizeDepsPlugin({
				exclude: ["@devo-ai/plugin", "@devo-ai/sdk", "@devo/configconv", "drizzle-orm"],
			}),
			copyDrizzleMigrations(),
		],
		resolve: {
			alias: {
				"@devo-ai/sdk/v2/client": sdkClientAlias,
				"@devo-ai/sdk/v2/protocol-validation": sdkProtocolValidationAlias,
			},
		},
		build: {
			rollupOptions: {
				input: { index: path.resolve(__dirname, "src/main/index.ts") },
			},
		},
	},
	preload: {
		// No externalizeDepsPlugin — sandboxed preloads must bundle all deps.
		// Output CJS because Electron sandboxed preloads cannot use ESM.
		resolve: {
			alias: {
				"@devo-ai/sdk/v2/client": sdkClientAlias,
				"@devo-ai/sdk/v2/protocol-validation": sdkProtocolValidationAlias,
			},
		},
		build: {
			rollupOptions: {
				input: { index: path.resolve(__dirname, "src/preload/index.ts") },
				output: {
					format: "cjs",
				},
			},
		},
	},
	renderer: {
		root: path.resolve(__dirname, "src/renderer"),
		plugins: [protocolTypesPlugin({ desktopDir: __dirname }), react(), tailwindcss()],
		resolve: {
			alias: {
				"@": path.resolve(__dirname, "src/renderer"),
				"@devo/ui": path.resolve(__dirname, "packages/ui/src"),
				"@devo-ai/sdk/v2/client": sdkClientAlias,
				"@devo-ai/sdk/v2/protocol-validation": sdkProtocolValidationAlias,
			},
		},
		worker: {
			format: "es",
		},
		server: {
			host: "127.0.0.1",
			port: 1420,
			strictPort: true,
		},
		build: {
			rollupOptions: {
				input: { index: path.resolve(__dirname, "src/renderer/index.html") },
			},
		},
	},
})
