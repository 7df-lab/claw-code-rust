// Import SDK types we reference in our own interfaces
import type {
	PermissionRequest as SdkPermissionRequest,
	QuestionRequest as SdkQuestionRequest,
} from "@devo-ai/sdk/v2/client"
import type { DesktopFolderStatus } from "../../preload/api"

// Re-export all SDK types from v2
export type {
	AssistantMessage,
	Event,
	EventMessagePartDelta,
	EventMessagePartUpdated,
	EventPermissionAsked,
	EventSessionCreated,
	EventSessionDeleted,
	EventSessionError,
	EventSessionStatus,
	EventSessionUpdated,
	FileDiff,
	FilePart,
	FilePartInput,
	Message,
	Part,
	PermissionRequest,
	PermissionResponse,
	Project as DevoProject,
	QuestionAnswer,
	QuestionInfo,
	QuestionOption,
	QuestionRequest,
	ReasoningPart,
	Session,
	SessionStatus,
	TextPart,
	Todo,
	ToolPart,
	ToolState,
	ToolStateCompleted,
	UserMessage,
	WorkspaceChangeAttribution,
	WorkspaceChangeBase,
	WorkspaceChangeCoverage,
	WorkspaceChangeScope,
	WorkspaceChangeSetStatus,
	WorkspaceChangeView,
	WorkspaceChangeViewStatus,
	WorkspaceChangedFile,
	WorkspaceChangedFileStatus,
	WorkspaceChangesReadResult,
	WorkspaceChangesUpdatedEventProperties,
	WorkspaceDiffDetail,
} from "@devo-ai/sdk/v2/client"

// ============================================================
// File attachment types
// ============================================================

/**
 * A file attachment ready to send — matches the shape returned by
 * PromptInput's onSubmit callback (FileUIPart from the `ai` package).
 */
export interface FileAttachment {
	type: "file"
	url: string
	mediaType?: string
	filename?: string
}

// ============================================================
// App-specific types
// ============================================================

/** An Devo server instance we're managing */
export interface ServerInstance {
	/** Unique ID for this server */
	id: string
	/** The project directory this server is for */
	directory: string
	/** URL of the running server */
	url: string
	/** Whether the server is healthy */
	connected: boolean
}

/** Where an agent runs */
export type EnvironmentType = "local" | "cloud" | "vm"

/** Derived agent status for UI display, mapped from SessionStatus */
export type AgentStatus = "running" | "waiting" | "paused" | "completed" | "failed" | "idle"

/** Project in the sidebar — aggregates from Devo projects */
export interface ProjectInfo {
	id: string
	name: string
	directory: string
	agentCount: number
}

/** Enriched project for the unified sidebar (includes directory for auto-start) */
export interface SidebarProject {
	/** Devo project ID (root commit hash) or hash of directory as fallback */
	id: string
	/** URL-safe slug: always `{name}-{id.slice(0,12)}` for stability */
	slug: string
	name: string
	directory: string
	agentCount: number
	lastActiveAt: number
	/** Whether at least one agent in this project is running or waiting for input */
	hasActiveAgent: boolean
	/** Filesystem status for user-managed Desktop folders. */
	folderStatus?: DesktopFolderStatus
}

/** Activity entry for the detail panel — derived from message parts */
export interface Activity {
	id: string
	timestamp: string
	type: "read" | "search" | "edit" | "run" | "think" | "write" | "tool"
	description: string
	detail?: string
}

/**
 * Agent is our UI-facing representation of an Devo session.
 * It merges Session data + SessionStatus + derived activity info.
 *
 * Note: Metrics (cost, tokens, work time, exchange count) are NOT included here.
 * They are expensive to compute (require iterating all messages + parts) and are
 * only needed by the SessionMetricsBar and command palette. Those components
 * subscribe to `sessionMetricsFamily` directly.
 */
export interface Agent {
	id: string
	name: string
	status: AgentStatus
	environment: EnvironmentType
	project: string
	/** URL slug for the project (for router navigation) */
	projectSlug: string
	/** Full project directory path (for auto-starting servers) */
	directory: string
	/** The root project directory. For worktree sessions this is the parent project,
	 *  for regular sessions it equals `directory`. Use this as the target for
	 *  apply-to-project and other operations that should target the main checkout. */
	projectDirectory: string
	branch: string
	/** Relative "last active" time, e.g. "5m" */
	duration: string
	currentActivity?: string
	activities: Activity[]
	/** The underlying Devo session ID */
	sessionId: string
	/** Pending permission requests for this agent */
	permissions: SdkPermissionRequest[]
	/** Pending question requests for this agent */
	questions: SdkQuestionRequest[]
	/** If set, this is a sub-agent spawned by the parent session */
	parentId?: string
	/** If set, the session runs in a git worktree at this root path */
	worktreePath?: string
	/** The branch name auto-created for the worktree (e.g. "devo/fix-auth-bug") */
	worktreeBranch?: string
	/** Timestamp (ms) of session creation — stable, never changes */
	createdAt: number
	/** Timestamp (ms) of last activity — for sorting and relative time display */
	lastActiveAt: number
	/** Renderer-local marker for a completed background turn waiting to be read. */
	hasUnreadCompletion?: boolean
}
