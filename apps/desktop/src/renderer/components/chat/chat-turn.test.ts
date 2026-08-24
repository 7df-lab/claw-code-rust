import { readFileSync } from "node:fs";
import { describe, expect, test } from "bun:test";

const source = readFileSync(
  new URL("./chat-turn.tsx", import.meta.url),
  "utf8",
);
const thoughtRowSource = readFileSync(
  new URL("./thought-row.tsx", import.meta.url),
  "utf8",
);
const transcriptDisclosureSource = readFileSync(
  new URL("./transcript-disclosure.tsx", import.meta.url),
  "utf8",
);
const processTimelineViewSource = readFileSync(
  new URL("./process-timeline-view.tsx", import.meta.url),
  "utf8",
);
const chatViewSource = readFileSync(
  new URL("./chat-view.tsx", import.meta.url),
  "utf8",
);
const chatToolCallSource = readFileSync(
  new URL("./chat-tool-call.tsx", import.meta.url),
  "utf8",
);
const compactionDividerSource = readFileSync(
  new URL("./compaction-status-divider.tsx", import.meta.url),
  "utf8",
);
const eventProcessorSource = readFileSync(
  new URL("../../atoms/actions/event-processor.ts", import.meta.url),
  "utf8",
);
const clientSource = readFileSync(
  new URL("../../../../packages/devo-ai-sdk/src/v2/client.ts", import.meta.url),
  "utf8",
);
const sharedReasoningSource = readFileSync(
  new URL(
    "../../../../packages/ui/src/components/ai-elements/reasoning.tsx",
    import.meta.url,
  ),
  "utf8",
);
const responseActionsProps =
  source.match(/\{!working && responseText && \([\s\S]*?<MessageActions([^>]*)>/)?.[1] ??
  "";
const footerMetadataSource =
  source.match(
    /\{\/\* Per-turn metadata[\s\S]*?\{\/\* Turn-level message actions/,
  )?.[0] ?? "";
const processTimelineIndex = source.indexOf("<ProcessTimelineView");

describe("ChatTurnComponent transcript controls", () => {
  test("keeps completed process collapsed, suppresses zero-second footer, and shows actions", () => {
    expect({
      collapsesCompletedProcess:
        source.includes("const processSectionVisible =") &&
        source.includes("completedProcessExpanded"),
      showsSubSecondDuration: source.includes(
        'if (workTimeMs <= 0) return ""',
      ) && source.includes("return formatWorkDuration(workTimeMs)"),
      usesActiveTurnDuration: source.includes(
        "computeTurnWorkTime(turn, { active: working })",
      ),
      usesInterleavedTimeline: source.includes("buildProcessTimeline"),
      omitsStepsToggle: !source.includes("const stepsToggle ="),
      footerConditionOmitsDuration: footerMetadataSource.includes(
        "turn.assistantMessages.length > 0 && (turnModel || turnCostStr)",
      ),
      footerDoesNotRenderDuration: !footerMetadataSource.includes(
        "{duration && <span>{duration}</span>}",
      ),
      usesAlwaysVisibleActions:
        responseActionsProps.includes('className="-ml-1"'),
      usesHoverHiddenActions:
        responseActionsProps.includes("opacity-0") ||
        responseActionsProps.includes("group-hover/turn:opacity-100"),
      rendersTimelineDirectlyUnderWorkedFor:
        processTimelineIndex !== -1 &&
        source.indexOf("<CompletedTurnProcessDisclosure") < processTimelineIndex,
    }).toEqual({
      collapsesCompletedProcess: true,
      showsSubSecondDuration: true,
      usesActiveTurnDuration: true,
      usesInterleavedTimeline: true,
      omitsStepsToggle: true,
      footerConditionOmitsDuration: true,
      footerDoesNotRenderDuration: true,
      usesAlwaysVisibleActions: true,
      usesHoverHiddenActions: false,
      rendersTimelineDirectlyUnderWorkedFor: true,
    });
  });

  test("wires pending permission requests into the active chat turn", () => {
    expect({
      chatTurnAcceptsPendingPermission: source.includes(
        "pendingPermission?: PendingPermission",
      ),
      chatTurnRendersPermissionItem:
        source.includes("<PermissionItem") &&
        source.includes("pendingPermission.request"),
      chatViewPassesPermissionToLastTurn: chatViewSource.includes(
        "pendingPermission={index === turns.length - 1 ? effectivePermission : undefined}",
      ),
      inputKeepsNoTurnPermissionFallback: chatViewSource.includes(
        "turns.length === 0 && effectivePermission",
      ),
      permissionReplyClearsPendingCard:
        chatViewSource.includes("removePermissionAtom") &&
        chatViewSource.includes(
          "removePermission({ sessionId: permissionSessionId, permissionId })",
        ),
      noTurnPermissionFallbackUsesClearingHandlers:
        chatViewSource.includes("onApprove={handleApprovePermission}") &&
        chatViewSource.includes("onDeny={handleDenyPermission}"),
      chatToolCallKeepsUnusedPermissionProp:
        chatToolCallSource.includes("permission?:"),
    }).toEqual({
      chatTurnAcceptsPendingPermission: true,
      chatTurnRendersPermissionItem: true,
      chatViewPassesPermissionToLastTurn: true,
      inputKeepsNoTurnPermissionFallback: true,
      permissionReplyClearsPendingCard: true,
      noTurnPermissionFallbackUsesClearingHandlers: true,
      chatToolCallKeepsUnusedPermissionProp: false,
    });
  });

  test("renders the active turn working timer between user message and assistant content", () => {
    const userMessageIndex = source.indexOf("{/* User message */}");
    const workingStripIndex = source.indexOf("<WorkingTurnStatusStrip");
    const processTimelineSectionIndex = source.indexOf(
      "Interleaved thought/tool process timeline",
    );
    const responseTextIndex = source.indexOf("{/* Streaming response");

    expect({
      definesWorkingStrip: source.includes("function WorkingTurnStatusStrip"),
      usesWorkingForCopy: source.includes("Working for {display}"),
      reusesTurnDuration: source.includes(
        "computeTurnWorkTime(turn, { active: true })",
      ),
      placesStripAfterUserMessage:
        userMessageIndex !== -1 &&
        workingStripIndex !== -1 &&
        userMessageIndex < workingStripIndex,
      placesStripBeforeProcessTimeline:
        workingStripIndex !== -1 &&
        processTimelineSectionIndex !== -1 &&
        responseTextIndex !== -1 &&
        workingStripIndex < processTimelineSectionIndex &&
        workingStripIndex < responseTextIndex,
      removesOldWorkingShimmer: !source.includes("Working shimmer"),
      keepsCompletedDurationAffordance: source.includes('Worked for "'),
    }).toEqual({
      definesWorkingStrip: true,
      usesWorkingForCopy: true,
      reusesTurnDuration: true,
      placesStripAfterUserMessage: true,
      placesStripBeforeProcessTimeline: true,
      removesOldWorkingShimmer: true,
      keepsCompletedDurationAffordance: true,
    });
  });

  test("folds completed process details under Worked for while keeping final answer visible", () => {
    const disclosureIndex = source.indexOf("<CompletedTurnProcessDisclosure");
    const processSectionIndex = source.indexOf(
      "Interleaved thought/tool process timeline",
    );
    const completedFinalResponseIndex = source.indexOf(
      "{/* Completed final response */}",
    );

    expect({
      definesProcessDisclosure: source.includes(
        "function CompletedTurnProcessDisclosure",
      ),
      tracksCompletedProcessState: source.includes(
        "const [completedProcessExpanded, setCompletedProcessExpanded] = useState(false)",
      ),
      splitsFinalResponseFromProcess:
        source.includes("splitCompletedTurnParts") &&
        source.includes("completedProcessParts") &&
        source.includes("finalResponsePart"),
      processSectionCanBeCollapsed:
        source.includes("const processSectionVisible =") &&
        source.includes("completedProcessExpanded"),
      disclosureReplacesCompletedDurationRow:
        disclosureIndex !== -1 && !source.includes("<CompletedTurnDurationRow"),
      finalResponseOutsideProcess:
        processSectionIndex !== -1 &&
        completedFinalResponseIndex !== -1 &&
        processSectionIndex < completedFinalResponseIndex,
      rendersThoughtRowsInTimeline: processTimelineViewSource.includes(
        "<ThoughtRow",
      ),
    }).toEqual({
      definesProcessDisclosure: true,
      tracksCompletedProcessState: true,
      splitsFinalResponseFromProcess: true,
      processSectionCanBeCollapsed: true,
      disclosureReplacesCompletedDurationRow: true,
      finalResponseOutsideProcess: true,
      rendersThoughtRowsInTimeline: true,
    });
  });

  test("keeps completed process disclosure reachable when duration is unavailable", () => {
    expect({
      disclosureAlwaysRendersWhenMounted: !source.includes(
        "if (!duration && !hasProcessDetails) return null",
      ),
      disclosureUsesWorkedFallback: source.includes(
        '{duration ? "Worked for " : "Worked"}',
      ),
      renderConditionUsesWorkedForSummary: source.includes(
        "!working && showWorkedForSummary",
      ),
      showsWorkedForOnAnyCompletedTurn: source.includes(
        "return turn.assistantMessages.length > 0",
      ),
    }).toEqual({
      disclosureAlwaysRendersWhenMounted: true,
      disclosureUsesWorkedFallback: true,
      renderConditionUsesWorkedForSummary: true,
      showsWorkedForOnAnyCompletedTurn: true,
    });
  });

  test("uses transcript disclosure rows for thoughts and tools", () => {
    expect({
      definesThoughtRow: thoughtRowSource.includes("export const ThoughtRow"),
      usesTranscriptDisclosureTrigger: transcriptDisclosureSource.includes(
        "export const TranscriptDisclosureTrigger",
      ),
      usesCollapsedThoughtChevron:
        transcriptDisclosureSource.includes("ChevronRightIcon") &&
        transcriptDisclosureSource.includes("ChevronDownIcon"),
      removesBareReasoningTrigger: !source.includes("<ReasoningTrigger />"),
      keepsSharedReasoningTriggerUnchanged:
        sharedReasoningSource.includes("export const ReasoningTrigger") &&
        sharedReasoningSource.includes("Thought for a few seconds"),
      dropsVisibleThoughtCopyDependency: !source.includes(
        "Thought for a few seconds",
      ),
      keepsActiveThinkingCue: thoughtRowSource.includes("Thinking..."),
      switchesToThoughtWhenComplete: thoughtRowSource.includes(
        "<span>Thought</span>",
      ),
      toolsUseTranscriptDisclosure: chatToolCallSource.includes(
        "<TranscriptDisclosure",
      ),
      toolsOmitDurationTrailing: !chatToolCallSource.includes("getToolDuration(part)"),
      timelineRendersSeparateThoughtRows: processTimelineViewSource.includes(
        'item.kind === "thought"',
      ),
      disclosureDoesNotShiftLeft:
        !transcriptDisclosureSource.includes("-mx-1.5") &&
        transcriptDisclosureSource.includes("px-0 py-0.5"),
      thoughtHasNoLeadingSpacer: !thoughtRowSource.includes(
        'leading={<span aria-hidden="true" className="size-3.5 shrink-0" />}',
      ),
      assistantColumnHasNoProcessIndent: !source.includes("pl-10"),
      hoverShowsExpandChevron:
        transcriptDisclosureSource.includes("group-hover/row:opacity-100") &&
        transcriptDisclosureSource.includes("group/row"),
      expandChevronFollowsLabel:
        transcriptDisclosureSource.includes("{label}</span>") &&
        transcriptDisclosureSource.includes("{chevron}") &&
        transcriptDisclosureSource.indexOf("{label}</span>") <
          transcriptDisclosureSource.indexOf("{chevron}"),
      toolsOmitLeadingIcons: !chatToolCallSource.includes("leading={"),
      completedWriteHidesSpinner: processTimelineViewSource.includes(
        "turnWorking={working}",
      ),
      gatesActionsUntilTurnFinishes: source.includes(
        "{!working && responseText && (",
      ),
    }).toEqual({
      definesThoughtRow: true,
      usesTranscriptDisclosureTrigger: true,
      usesCollapsedThoughtChevron: true,
      removesBareReasoningTrigger: true,
      keepsSharedReasoningTriggerUnchanged: true,
      dropsVisibleThoughtCopyDependency: true,
      keepsActiveThinkingCue: true,
      switchesToThoughtWhenComplete: true,
      toolsUseTranscriptDisclosure: true,
      toolsOmitDurationTrailing: true,
      timelineRendersSeparateThoughtRows: true,
      disclosureDoesNotShiftLeft: true,
      thoughtHasNoLeadingSpacer: true,
      assistantColumnHasNoProcessIndent: true,
      hoverShowsExpandChevron: true,
      expandChevronFollowsLabel: true,
      toolsOmitLeadingIcons: true,
      completedWriteHidesSpinner: true,
      gatesActionsUntilTurnFinishes: true,
    });
  });

  test("renders compaction lifecycle as a transcript divider", () => {
    expect({
      filtersStartedTextFromAssistantResponse:
        source.includes("isCompactionStatusText(part.text)") &&
        source.includes("continue"),
      rendersDividerAfterResponse: source.includes(
        "<CompactionStatusDivider status={displayedCompactionStatus} />",
      ),
      updatesMemoWhenCompactionStatusChanges: source.includes(
        "prev.compactionStatus !== next.compactionStatus",
      ),
      chatViewPassesSessionCompactionStatus:
        chatViewSource.includes("compactionStatusFamily(agent.sessionId)") &&
        chatViewSource.includes("compactionStatus={compactionStatus}"),
      usesRequestedIcons:
        compactionDividerSource.includes("BubblesIcon") &&
        compactionDividerSource.includes("PackageCheckIcon"),
      usesRequestedLabels:
        compactionDividerSource.includes("Compacting context") &&
        compactionDividerSource.includes("Context compacted"),
      keepsIconStyleConsistent:
        compactionDividerSource.includes("size-3.5") &&
        compactionDividerSource.includes("stroke-[1.5]"),
      handlesCompactionEvents:
        eventProcessorSource.includes("session.compaction.started") &&
        eventProcessorSource.includes("session.compaction.completed") &&
        eventProcessorSource.includes("session.compaction.failed"),
      bridgesRuntimeCompactionEvents:
        clientSource.includes("sessionCompactionFromOriginalEvent") &&
        clientSource.includes("sessionIdFromCompactionPayload") &&
        clientSource.includes("SessionCompactionCompleted") &&
        clientSource.includes("session.compaction.${compaction.status}") &&
        clientSource.includes("upsertCompaction") &&
        clientSource.includes("contextCompaction"),
    }).toEqual({
      filtersStartedTextFromAssistantResponse: true,
      rendersDividerAfterResponse: true,
      updatesMemoWhenCompactionStatusChanges: true,
      chatViewPassesSessionCompactionStatus: true,
      usesRequestedIcons: true,
      usesRequestedLabels: true,
      keepsIconStyleConsistent: true,
      handlesCompactionEvents: true,
      bridgesRuntimeCompactionEvents: true,
    });
  });

  test("interleaves thoughts and tools inside the process timeline", () => {
    expect({
      noMergedTurnThinkingSection: !source.includes("function TurnThinkingSection"),
      noReasoningProcessGroups: !source.includes('"reasoning-process"'),
      usesPerRowExpansion: source.includes("expandedRowIds"),
      endsThinkingWhenAssistantTextStarts: processTimelineViewSource.includes(
        "isReasoningPartActivelyStreaming",
      ),
      keepsWorkedForOnReasoningOnlyTurns: source.includes("showWorkedForSummary"),
      verboseUsesDisplayModeOnly: source.includes(
        'const showVerboseTools = displayMode === "verbose"',
      ),
      keepsWorkExpandedWhileRunning: source.includes(
        "(working && processTimelineItems.length > 0)",
      ),
      collapsesWorkWhenIdle: source.includes(
        "(!working && hasCompletedProcessDetails && completedProcessExpanded)",
      ),
      keepsInnerRowsCollapsed:
        !source.includes("isProcessItemStreaming") &&
        source.includes("setExpandedRowIds(new Set())"),
    }).toEqual({
      noMergedTurnThinkingSection: true,
      noReasoningProcessGroups: true,
      usesPerRowExpansion: true,
      endsThinkingWhenAssistantTextStarts: true,
      keepsWorkedForOnReasoningOnlyTurns: true,
      verboseUsesDisplayModeOnly: true,
      keepsWorkExpandedWhileRunning: true,
      collapsesWorkWhenIdle: true,
      keepsInnerRowsCollapsed: true,
    });
  });

  test("renders plan items with a dedicated plan card and actions", () => {
    const planBlockSource = readFileSync(
      new URL("./plan-block.tsx", import.meta.url),
      "utf8",
    );
    expect({
      planBlock: planBlockSource.includes("Proposed Plan") && planBlockSource.includes("Implement Plan"),
      chatTurnUsesPlanBlock: source.includes("<PlanBlock") || source.includes("<AssistantTextBlock"),
      chatViewImplement: chatViewSource.includes('collaborationMode: "build"') && chatViewSource.includes("Implement Plan"),
      modeToggle: chatViewSource.includes("Toggle plan mode"),
      skillsSlash: chatViewSource.includes('case "skills":'),
    }).toEqual({
      planBlock: true,
      chatTurnUsesPlanBlock: true,
      chatViewImplement: true,
      modeToggle: true,
      skillsSlash: true,
    });
  });
});
