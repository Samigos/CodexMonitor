import { convertFileSrc } from "@tauri-apps/api/core";
import type { ConversationItem } from "../../../types";

export type ToolSummary = {
  label: string;
  value?: string;
  detail?: string;
  output?: string;
};

export type StatusTone = "completed" | "processing" | "failed" | "unknown";

export type ParsedReasoning = {
  summaryTitle: string;
  bodyText: string;
  hasBody: boolean;
  workingLabel: string | null;
};

export type MessageImage = {
  src: string;
  label: string;
};

export type ToolGroupItem = Extract<
  ConversationItem,
  { kind: "tool" | "reasoning" | "explore" | "userInput" }
>;

type FileChangeToolItem = Extract<ConversationItem, { kind: "tool" }> & {
  toolType: "fileChange";
};

export type ToolGroup = {
  id: string;
  items: ToolGroupItem[];
  toolCount: number;
  messageCount: number;
};

export type FileChangeSummaryEdit = {
  id: string;
  label: string;
  diff: string;
  additions: number;
  deletions: number;
};

export type FileChangeSummaryFile = {
  id: string;
  path: string;
  status: "A" | "D" | "M" | "R";
  edits: FileChangeSummaryEdit[];
};

export type FileChangeSummary = {
  id: string;
  files: FileChangeSummaryFile[];
  counts: {
    added: number;
    deleted: number;
    modified: number;
    renamed: number;
  };
};

export type MessageListEntry =
  | { kind: "item"; item: ConversationItem }
  | { kind: "toolGroup"; group: ToolGroup };

export const SCROLL_THRESHOLD_PX = 120;
export const MAX_COMMAND_OUTPUT_LINES = 200;

export function basename(path: string) {
  if (!path) {
    return "";
  }
  const normalized = path.replace(/\\/g, "/");
  const parts = normalized.split("/").filter(Boolean);
  return parts.length ? parts[parts.length - 1] : path;
}

function parseToolArgs(detail: string) {
  if (!detail) {
    return null;
  }
  try {
    return JSON.parse(detail) as Record<string, unknown>;
  } catch {
    return null;
  }
}

function firstStringField(
  source: Record<string, unknown> | null,
  keys: string[],
) {
  if (!source) {
    return "";
  }
  for (const key of keys) {
    const value = source[key];
    if (typeof value === "string" && value.trim()) {
      return value.trim();
    }
  }
  return "";
}

function formatCollabAgentLabel(agent: {
  threadId: string;
  nickname?: string;
  role?: string;
}) {
  const nickname = agent.nickname?.trim();
  const role = agent.role?.trim();
  if (nickname && role) {
    return `${nickname} [${role}]`;
  }
  if (nickname) {
    return nickname;
  }
  if (role) {
    return `${agent.threadId} [${role}]`;
  }
  return agent.threadId;
}

function summarizeCollabLabel(title: string, status?: string) {
  const tool = title.replace(/^collab:\s*/i, "").trim().toLowerCase();
  const tone = statusToneFromText(status);
  if (tool.includes("wait")) {
    return tone === "processing" ? "waiting for" : "waited for";
  }
  if (tool.includes("resume")) {
    return tone === "processing" ? "resuming" : "resumed";
  }
  if (tool.includes("close")) {
    return tone === "processing" ? "closing" : "closed";
  }
  if (tool.includes("spawn")) {
    return tone === "processing" ? "spawning" : "spawned";
  }
  if (tool.includes("send") || tool.includes("interaction")) {
    return tone === "processing" ? "sending to" : "sent to";
  }
  return "sub-agent";
}

function summarizeCollabReceiver(
  item: Extract<ConversationItem, { kind: "tool" }>,
) {
  const receivers =
    item.collabReceivers && item.collabReceivers.length > 0
      ? item.collabReceivers
      : item.collabReceiver
        ? [item.collabReceiver]
        : [];
  if (receivers.length === 0) {
    return item.title || "";
  }
  if (receivers.length === 1) {
    return formatCollabAgentLabel(receivers[0]);
  }
  return `${formatCollabAgentLabel(receivers[0])} +${receivers.length - 1}`;
}

export function toolNameFromTitle(title: string) {
  if (!title.toLowerCase().startsWith("tool:")) {
    return "";
  }
  const [, toolPart = ""] = title.split(":");
  const segments = toolPart.split("/").map((segment) => segment.trim());
  return segments.length ? segments[segments.length - 1] : "";
}

export function formatCount(value: number, singular: string, plural: string) {
  return `${value} ${value === 1 ? singular : plural}`;
}

function sanitizeReasoningTitle(title: string) {
  return title
    .replace(/[`*_~]/g, "")
    .replace(/\[(.*?)\]\(.*?\)/g, "$1")
    .trim();
}

export function parseReasoning(
  item: Extract<ConversationItem, { kind: "reasoning" }>,
): ParsedReasoning {
  const summary = item.summary ?? "";
  const content = item.content ?? "";
  const hasSummary = summary.trim().length > 0;
  const titleSource = hasSummary ? summary : content;
  const titleLines = titleSource.split("\n");
  const trimmedLines = titleLines.map((line) => line.trim());
  const titleLineIndex = trimmedLines.findIndex(Boolean);
  const rawTitle = titleLineIndex >= 0 ? trimmedLines[titleLineIndex] : "";
  const cleanTitle = sanitizeReasoningTitle(rawTitle);
  const summaryTitle = cleanTitle
    ? cleanTitle.length > 80
      ? `${cleanTitle.slice(0, 80)}…`
      : cleanTitle
    : "Reasoning";
  const summaryLines = summary.split("\n");
  const contentLines = content.split("\n");
  const summaryBody =
    hasSummary && titleLineIndex >= 0
      ? summaryLines
          .filter((_, index) => index !== titleLineIndex)
          .join("\n")
          .trim()
      : "";
  const contentBody = hasSummary
    ? content.trim()
    : titleLineIndex >= 0
      ? contentLines
          .filter((_, index) => index !== titleLineIndex)
          .join("\n")
          .trim()
      : content.trim();
  const bodyParts = [summaryBody, contentBody].filter(Boolean);
  const bodyText = bodyParts.join("\n\n").trim();
  const hasBody = bodyText.length > 0;
  const hasAnyText = titleSource.trim().length > 0;
  const workingLabel = hasAnyText ? summaryTitle : null;
  return {
    summaryTitle,
    bodyText,
    hasBody,
    workingLabel,
  };
}

export function normalizeMessageImageSrc(path: string) {
  if (!path) {
    return "";
  }
  if (path.startsWith("data:") || path.startsWith("http://") || path.startsWith("https://")) {
    return path;
  }
  if (path.startsWith("file://")) {
    return path;
  }
  try {
    return convertFileSrc(path);
  } catch {
    return "";
  }
}

function isToolGroupItem(item: ConversationItem): item is ToolGroupItem {
  return (
    item.kind === "tool" ||
    item.kind === "reasoning" ||
    item.kind === "explore" ||
    item.kind === "userInput"
  );
}

function isFileChangeToolItem(
  item: ConversationItem,
): item is FileChangeToolItem {
  return item.kind === "tool" && item.toolType === "fileChange";
}

function normalizeChangePath(rawPath: string) {
  let normalized = rawPath.trim().replace(/\\/g, "/");
  normalized = normalized.replace(/^\.\/+/, "");
  normalized = normalized.replace(/^(?:a|b)\//, "");
  normalized = normalized.replace(/\/+/g, "/");
  return normalized;
}

function extractPathFromDiff(diff: string): string | null {
  if (!diff.trim()) {
    return null;
  }

  const lines = diff.split("\n");

  for (const line of lines) {
    const match = /^diff --git a\/(.+?) b\/(.+)$/.exec(line.trim());
    if (match?.[2]) {
      return normalizeChangePath(match[2]);
    }
  }

  for (const line of lines) {
    const match = /^\+\+\+ (?:b\/)?(.+)$/.exec(line.trim());
    if (!match?.[1]) {
      continue;
    }
    const path = normalizeChangePath(match[1]);
    if (path && path !== "/dev/null") {
      return path;
    }
  }

  return null;
}

function mapChangeKindToStatus(kind?: string): FileChangeSummaryFile["status"] {
  const normalized = (kind ?? "").trim().toLowerCase();
  if (normalized === "add" || normalized === "added" || normalized === "create") {
    return "A";
  }
  if (normalized === "delete" || normalized === "deleted" || normalized === "remove") {
    return "D";
  }
  if (normalized === "rename" || normalized === "renamed") {
    return "R";
  }
  return "M";
}

function countDiffStats(diff: string) {
  let additions = 0;
  let deletions = 0;

  for (const line of diff.split("\n")) {
    if (!line) {
      continue;
    }
    if (
      line.startsWith("+++")
      || line.startsWith("---")
      || line.startsWith("diff --git")
      || line.startsWith("@@")
      || line.startsWith("index ")
      || line.startsWith("\\ No newline")
    ) {
      continue;
    }
    if (line.startsWith("+")) {
      additions += 1;
      continue;
    }
    if (line.startsWith("-")) {
      deletions += 1;
    }
  }

  return { additions, deletions };
}

function incrementFileChangeCount(
  counts: FileChangeSummary["counts"],
  status: FileChangeSummaryFile["status"],
) {
  if (status === "A") {
    counts.added += 1;
  } else if (status === "D") {
    counts.deleted += 1;
  } else if (status === "R") {
    counts.renamed += 1;
  } else {
    counts.modified += 1;
  }
}

function decrementFileChangeCount(
  counts: FileChangeSummary["counts"],
  status: FileChangeSummaryFile["status"],
) {
  if (status === "A") {
    counts.added = Math.max(0, counts.added - 1);
  } else if (status === "D") {
    counts.deleted = Math.max(0, counts.deleted - 1);
  } else if (status === "R") {
    counts.renamed = Math.max(0, counts.renamed - 1);
  } else {
    counts.modified = Math.max(0, counts.modified - 1);
  }
}

function buildFileChangeSummary(
  items: FileChangeToolItem[],
): FileChangeSummary | null {
  if (items.length === 0) {
    return null;
  }

  const filesByPath = new Map<string, FileChangeSummaryFile>();
  const editCountByPath = new Map<string, number>();
  const counts = {
    added: 0,
    deleted: 0,
    modified: 0,
    renamed: 0,
  };

  for (const item of items) {
    const changes = Array.isArray(item.changes) ? item.changes : [];
    for (const [changeIndex, change] of changes.entries()) {
      const pathFromChange = normalizeChangePath(change.path ?? "");
      const diff = change.diff ?? "";
      const path = extractPathFromDiff(diff) ?? pathFromChange;
      if (!path) {
        continue;
      }
      const status = mapChangeKindToStatus(change.kind);
      const existing = filesByPath.get(path);
      if (!existing) {
        filesByPath.set(path, {
          id: `file-change-${path}`,
          path,
          status,
          edits: [],
        });
        incrementFileChangeCount(counts, status);
      } else if (existing.status !== status) {
        decrementFileChangeCount(counts, existing.status);
        existing.status = status;
        incrementFileChangeCount(counts, status);
      }

      if (!diff.trim()) {
        continue;
      }

      const nextCount = (editCountByPath.get(path) ?? 0) + 1;
      editCountByPath.set(path, nextCount);
      const file = filesByPath.get(path);
      if (!file) {
        continue;
      }
      const { additions, deletions } = countDiffStats(diff);
      file.edits.push({
        id: `${path}@@${item.id}@@${changeIndex}`,
        label: `Edit ${nextCount}`,
        diff,
        additions,
        deletions,
      });
    }
  }

  const files = Array.from(filesByPath.values());
  if (files.length === 0) {
    return null;
  }

  return {
    id: `file-change-summary-${items[0]?.id ?? "thread"}`,
    files,
    counts,
  };
}

function splitTurnDiffIntoBlocks(diff: string) {
  if (!diff.trim()) {
    return [];
  }

  const lines = diff.split("\n");
  const blocks: string[] = [];
  let current: string[] = [];

  for (const line of lines) {
    if (line.startsWith("diff --git ")) {
      if (current.length > 0) {
        blocks.push(current.join("\n"));
      }
      current = [line];
      continue;
    }
    if (current.length > 0) {
      current.push(line);
    }
  }

  if (current.length > 0) {
    blocks.push(current.join("\n"));
  }

  if (blocks.length > 0) {
    return blocks;
  }

  return [diff];
}

function statusFromTurnDiffBlock(diff: string): FileChangeSummaryFile["status"] {
  const lines = diff.split("\n");
  if (
    lines.some((line) => line.startsWith("new file mode"))
    || lines.some((line) => line.startsWith("--- /dev/null"))
  ) {
    return "A";
  }
  if (
    lines.some((line) => line.startsWith("deleted file mode"))
    || lines.some((line) => line.startsWith("+++ /dev/null"))
  ) {
    return "D";
  }
  if (
    lines.some((line) => line.startsWith("rename from "))
    || lines.some((line) => line.startsWith("rename to "))
  ) {
    return "R";
  }
  return "M";
}

export function buildFileChangeSummaryFromTurnDiff(
  diff: string | null | undefined,
): FileChangeSummary | null {
  if (!diff?.trim()) {
    return null;
  }

  const filesByPath = new Map<string, FileChangeSummaryFile>();
  const editCountByPath = new Map<string, number>();
  const counts = {
    added: 0,
    deleted: 0,
    modified: 0,
    renamed: 0,
  };

  for (const block of splitTurnDiffIntoBlocks(diff)) {
    const path = extractPathFromDiff(block);
    if (!path) {
      continue;
    }
    const status = statusFromTurnDiffBlock(block);
    const existing = filesByPath.get(path);
    if (!existing) {
      filesByPath.set(path, {
        id: `turn-diff-${path}`,
        path,
        status,
        edits: [],
      });
      incrementFileChangeCount(counts, status);
    } else if (existing.status !== status) {
      decrementFileChangeCount(counts, existing.status);
      existing.status = status;
      incrementFileChangeCount(counts, status);
    }

    const nextCount = (editCountByPath.get(path) ?? 0) + 1;
    editCountByPath.set(path, nextCount);
    const file = filesByPath.get(path);
    if (!file) {
      continue;
    }
    const { additions, deletions } = countDiffStats(block);
    file.edits.push({
      id: `${path}@@turn-diff@@${nextCount}`,
      label: `Edit ${nextCount}`,
      diff: block,
      additions,
      deletions,
    });
  }

  const files = Array.from(filesByPath.values());
  if (files.length === 0) {
    return null;
  }

  return {
    id: "file-change-summary-turn-diff",
    files,
    counts,
  };
}

export function buildLatestFileChangeSummary(
  items: ConversationItem[],
  turnDiff?: string | null,
): FileChangeSummary | null {
  const turnDiffSummary = buildFileChangeSummaryFromTurnDiff(turnDiff);
  if (turnDiffSummary) {
    return turnDiffSummary;
  }

  let latestSummary: FileChangeSummary | null = null;
  let buffer: ToolGroupItem[] = [];

  for (let index = 0; index < items.length; index += 1) {
    const item = items[index];
    if (isToolGroupItem(item)) {
      buffer.push(item);
      continue;
    }

    if (item.kind === "message" && item.role === "assistant") {
      const trailingItems: ToolGroupItem[] = [];
      let nextIndex = index + 1;
      while (nextIndex < items.length && isToolGroupItem(items[nextIndex]!)) {
        trailingItems.push(items[nextIndex] as ToolGroupItem);
        nextIndex += 1;
      }

      const fileChangeItems = [...buffer, ...trailingItems].filter(isFileChangeToolItem);
      const summary = buildFileChangeSummary(fileChangeItems);
      if (summary) {
        latestSummary = summary;
      }

      buffer = [];
      index = nextIndex - 1;
      continue;
    }

    buffer = [];
  }

  return latestSummary;
}

function mergeExploreItems(
  items: Extract<ConversationItem, { kind: "explore" }>[],
): Extract<ConversationItem, { kind: "explore" }> {
  const first = items[0];
  const last = items[items.length - 1];
  const status = last?.status ?? "explored";
  const entries = items.flatMap((item) => item.entries);
  return {
    id: first.id,
    kind: "explore",
    status,
    entries,
  };
}

function mergeConsecutiveExploreRuns(items: ToolGroupItem[]): ToolGroupItem[] {
  const result: ToolGroupItem[] = [];
  let run: Extract<ConversationItem, { kind: "explore" }>[] = [];

  const flushRun = () => {
    if (run.length === 0) {
      return;
    }
    if (run.length === 1) {
      result.push(run[0]);
    } else {
      result.push(mergeExploreItems(run));
    }
    run = [];
  };

  items.forEach((item) => {
    if (item.kind === "explore") {
      run.push(item);
      return;
    }
    flushRun();
    result.push(item);
  });
  flushRun();
  return result;
}

export function buildToolGroups(items: ConversationItem[]): MessageListEntry[] {
  const entries: MessageListEntry[] = [];
  let buffer: ToolGroupItem[] = [];

  const buildToolEntries = (items: ToolGroupItem[]) => {
    if (items.length === 0) {
      return [];
    }
    const normalizedBuffer = mergeConsecutiveExploreRuns(items);
    const toolCount = normalizedBuffer.reduce((total, item) => {
      if (item.kind === "tool") {
        return total + 1;
      }
      if (item.kind === "explore") {
        return total + item.entries.length;
      }
      return total;
    }, 0);
    const messageCount = normalizedBuffer.filter(
      (item) => item.kind !== "tool" && item.kind !== "explore",
    ).length;
    const nextEntries: MessageListEntry[] = [];
    if (toolCount === 0 || normalizedBuffer.length === 1) {
      normalizedBuffer.forEach((item) => nextEntries.push({ kind: "item", item }));
    } else {
      nextEntries.push({
        kind: "toolGroup",
        group: {
          id: normalizedBuffer[0].id,
          items: normalizedBuffer,
          toolCount,
          messageCount,
        },
      });
    }
    return nextEntries;
  };

  const flush = () => {
    if (buffer.length === 0) {
      return;
    }
    entries.push(...buildToolEntries(buffer));
    buffer = [];
  };

  items.forEach((item) => {
    if (isToolGroupItem(item)) {
      buffer.push(item);
    } else {
      flush();
      entries.push({ kind: "item", item });
    }
  });
  flush();
  return entries;
}

export function cleanCommandText(commandText: string) {
  if (!commandText) {
    return "";
  }
  const trimmed = commandText.trim();
  const shellMatch = trimmed.match(
    /^(?:\/\S+\/)?(?:bash|zsh|sh|fish)(?:\.exe)?\s+-lc\s+(['"])([\s\S]+)\1$/,
  );
  const inner = shellMatch ? shellMatch[2] : trimmed;
  const cdMatch = inner.match(
    /^\s*cd\s+[^&;]+(?:\s*&&\s*|\s*;\s*)([\s\S]+)$/i,
  );
  const stripped = cdMatch ? cdMatch[1] : inner;
  return stripped.trim();
}

export function buildToolSummary(
  item: Extract<ConversationItem, { kind: "tool" }>,
  commandText: string,
): ToolSummary {
  if (item.toolType === "commandExecution") {
    const cleanedCommand = cleanCommandText(commandText);
    return {
      label: "command",
      value: cleanedCommand || "Command",
      detail: "",
      output: item.output || "",
    };
  }

  if (item.toolType === "webSearch") {
    return {
      label: statusToneFromText(item.status) === "processing" ? "searching" : "searched",
      value: item.detail || "the web",
    };
  }

  if (item.toolType === "imageView") {
    const file = basename(item.detail || "");
    return {
      label: "read",
      value: file || "image",
    };
  }

  if (item.toolType === "hook") {
    return {
      label: "hook",
      value: item.title.replace(/^Hook:\s*/i, "").trim() || item.title || "hook",
      detail: item.detail || "",
      output: item.output || "",
    };
  }

  if (item.toolType === "collabToolCall") {
    return {
      label: summarizeCollabLabel(item.title, item.status),
      value: summarizeCollabReceiver(item),
      detail: item.detail || "",
      output: item.output || "",
    };
  }

  if (item.toolType === "mcpToolCall") {
    const toolName = toolNameFromTitle(item.title);
    const args = parseToolArgs(item.detail);
    if (toolName.toLowerCase().includes("search")) {
      return {
        label: statusToneFromText(item.status) === "processing" ? "searching" : "searched",
        value:
          firstStringField(args, ["query", "pattern", "text"]) || item.detail,
      };
    }
    if (toolName.toLowerCase().includes("read")) {
      const targetPath =
        firstStringField(args, ["path", "file", "filename"]) || item.detail;
      return {
        label: "read",
        value: basename(targetPath),
        detail: targetPath && targetPath !== basename(targetPath) ? targetPath : "",
      };
    }
    if (toolName) {
      return {
        label: "tool",
        value: toolName,
        detail: item.detail || "",
      };
    }
  }

  return {
    label: "tool",
    value: item.title || "",
    detail: item.detail || "",
    output: item.output || "",
  };
}

export function formatDurationMs(durationMs: number) {
  const durationSeconds = Math.max(0, Math.floor(durationMs / 1000));
  const durationMinutes = Math.floor(durationSeconds / 60);
  const durationRemainder = durationSeconds % 60;
  return `${durationMinutes}:${String(durationRemainder).padStart(2, "0")}`;
}

export function statusToneFromText(status?: string): StatusTone {
  if (!status) {
    return "unknown";
  }
  const normalized = status.toLowerCase();
  if (/(fail|error)/.test(normalized)) {
    return "failed";
  }
  if (/(pending|running|processing|started|in[_\s-]?progress)/.test(normalized)) {
    return "processing";
  }
  if (/(complete|completed|success|done)/.test(normalized)) {
    return "completed";
  }
  return "unknown";
}

export function toolStatusTone(
  item: Extract<ConversationItem, { kind: "tool" }>,
  hasChanges: boolean,
): StatusTone {
  const fromStatus = statusToneFromText(item.status);
  if (fromStatus !== "unknown") {
    return fromStatus;
  }
  if (item.output || hasChanges) {
    return "completed";
  }
  return "processing";
}

export function formatToolStatusLabel(
  item: Extract<ConversationItem, { kind: "tool" }>,
) {
  if (item.toolType !== "hook") {
    return "";
  }
  const parts: string[] = [];
  const status = (item.status ?? "").trim().toLowerCase();
  if (status) {
    parts.push(status.replace(/[_-]+/g, " "));
  }
  if (typeof item.durationMs === "number" && Number.isFinite(item.durationMs)) {
    parts.push(formatDurationMs(item.durationMs));
  }
  return parts.join(" • ");
}


export type PlanFollowupState = {
  shouldShow: boolean;
  planItemId: string | null;
};

export function computePlanFollowupState({
  threadId,
  items,
  isThinking,
  hasVisibleUserInputRequest,
}: {
  threadId: string | null;
  items: ConversationItem[];
  isThinking: boolean;
  hasVisibleUserInputRequest: boolean;
}): PlanFollowupState {
  if (!threadId) {
    return { shouldShow: false, planItemId: null };
  }
  if (hasVisibleUserInputRequest) {
    return { shouldShow: false, planItemId: null };
  }

  let planIndex = -1;
  let planItem: Extract<ConversationItem, { kind: "tool" }> | null = null;
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const item = items[index];
    if (item.kind === "tool" && item.toolType === "plan") {
      planIndex = index;
      planItem = item;
      break;
    }
  }

  if (!planItem) {
    return { shouldShow: false, planItemId: null };
  }

  const planItemId = planItem.id;

  if (!(planItem.output ?? "").trim()) {
    return { shouldShow: false, planItemId };
  }

  const planTone = toolStatusTone(planItem, false);
  if (planTone === "failed") {
    return { shouldShow: false, planItemId };
  }

  // Some backends stream plan output deltas without a final status update. As
  // soon as the turn stops thinking, treat the latest plan output as ready.
  if (isThinking && planTone !== "completed") {
    return { shouldShow: false, planItemId };
  }

  for (let index = planIndex + 1; index < items.length; index += 1) {
    const item = items[index];
    if (item.kind === "message" && item.role === "user") {
      return { shouldShow: false, planItemId };
    }
  }

  return { shouldShow: true, planItemId };
}

export function scrollKeyForItems(items: ConversationItem[]) {
  if (!items.length) {
    return "empty";
  }
  const last = items[items.length - 1];
  switch (last.kind) {
    case "message":
      return `${last.id}-${last.text.length}`;
    case "userInput":
      return `${last.id}-${last.status}-${last.questions.length}`;
    case "reasoning":
      return `${last.id}-${last.summary.length}-${last.content.length}`;
    case "explore":
      return `${last.id}-${last.status}-${last.entries.length}`;
    case "tool":
      return `${last.id}-${last.status ?? ""}-${last.output?.length ?? 0}`;
    case "diff":
      return `${last.id}-${last.status ?? ""}-${last.diff.length}`;
    case "review":
      return `${last.id}-${last.state}-${last.text.length}`;
    default: {
      const _exhaustive: never = last;
      return _exhaustive;
    }
  }
}

export function exploreKindLabel(
  kind: Extract<ConversationItem, { kind: "explore" }>["entries"][number]["kind"],
) {
  return kind[0].toUpperCase() + kind.slice(1);
}
