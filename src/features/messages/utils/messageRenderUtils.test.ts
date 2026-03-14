import { describe, expect, it } from "vitest";
import type { ConversationItem } from "../../../types";
import {
  buildFileChangeSummaryFromTurnDiff,
  buildLatestFileChangeSummary,
  buildToolGroups,
  buildToolSummary,
  statusToneFromText,
} from "./messageRenderUtils";

function makeToolItem(
  overrides: Partial<Extract<ConversationItem, { kind: "tool" }>>,
): Extract<ConversationItem, { kind: "tool" }> {
  return {
    id: "tool-1",
    kind: "tool",
    toolType: "webSearch",
    title: "Web search",
    detail: "codex monitor",
    status: "completed",
    output: "",
    ...overrides,
  };
}

describe("messageRenderUtils", () => {
  it("renders web search as searching while in progress", () => {
    const summary = buildToolSummary(makeToolItem({ status: "inProgress" }), "");
    expect(summary.label).toBe("searching");
    expect(summary.value).toBe("codex monitor");
  });

  it("renders mcp search calls as searching while in progress", () => {
    const summary = buildToolSummary(
      makeToolItem({
        toolType: "mcpToolCall",
        title: "Tool: web / search_query",
        detail: '{\n  "query": "codex monitor"\n}',
        status: "inProgress",
      }),
      "",
    );
    expect(summary.label).toBe("searching");
    expect(summary.value).toBe("codex monitor");
  });

  it("classifies camelCase inProgress as processing", () => {
    expect(statusToneFromText("inProgress")).toBe("processing");
  });

  it("renders collab tool calls with nickname and role", () => {
    const summary = buildToolSummary(
      makeToolItem({
        toolType: "collabToolCall",
        title: "Collab: wait",
        detail: "From thread-parent → thread-child",
        status: "completed",
        output: "Robie [explorer]: completed",
        collabReceivers: [
          {
            threadId: "thread-child",
            nickname: "Robie",
            role: "explorer",
          },
        ],
      }),
      "",
    );
    expect(summary.label).toBe("waited for");
    expect(summary.value).toBe("Robie [explorer]");
    expect(summary.output).toContain("Robie [explorer]: completed");
  });

  it("keeps file changes inline even after an assistant reply", () => {
    const items: ConversationItem[] = [
      {
        id: "user-1",
        kind: "message",
        role: "user",
        text: "Update the file",
      },
      {
        id: "change-1",
        kind: "tool",
        toolType: "fileChange",
        title: "File changes",
        detail: "M src/main.ts",
        status: "completed",
        changes: [
          {
            path: "src/main.ts",
            kind: "modify",
            diff:
              "diff --git a/src/main.ts b/src/main.ts\n--- a/src/main.ts\n+++ b/src/main.ts\n@@ -1 +1 @@\n-old\n+new",
          },
        ],
      },
      {
        id: "assistant-1",
        kind: "message",
        role: "assistant",
        text: "Done.",
      },
    ];

    const entries = buildToolGroups(items);

    expect(entries.map((entry) => entry.kind)).toEqual(["item", "item", "item"]);
    expect(entries[1]).toMatchObject({
      kind: "item",
      item: { kind: "tool", toolType: "fileChange" },
    });
  });

  it("builds the latest file summary from explicit fileChange items", () => {
    const items: ConversationItem[] = [
      {
        id: "user-1",
        kind: "message",
        role: "user",
        text: "Update the file",
      },
      {
        id: "change-1",
        kind: "tool",
        toolType: "fileChange",
        title: "File changes",
        detail: "M src/main.ts",
        status: "completed",
        changes: [
          {
            path: "src/main.ts",
            kind: "modify",
            diff:
              "diff --git a/src/main.ts b/src/main.ts\n--- a/src/main.ts\n+++ b/src/main.ts\n@@ -1 +1 @@\n-old\n+new",
          },
        ],
      },
      {
        id: "assistant-1",
        kind: "message",
        role: "assistant",
        text: "Done.",
      },
    ];

    const summary = buildLatestFileChangeSummary(items);

    expect(summary).toMatchObject({
      files: [{ path: "src/main.ts", status: "M" }],
    });
  });

  it("builds the latest file summary when file changes arrive after the assistant reply", () => {
    const items: ConversationItem[] = [
      {
        id: "user-1",
        kind: "message",
        role: "user",
        text: "Update the file",
      },
      {
        id: "assistant-1",
        kind: "message",
        role: "assistant",
        text: "Done.",
      },
      {
        id: "change-1",
        kind: "tool",
        toolType: "fileChange",
        title: "File changes",
        detail: "A src/new.ts",
        status: "completed",
        changes: [
          {
            path: "src/new.ts",
            kind: "add",
            diff:
              "diff --git a/src/new.ts b/src/new.ts\n--- /dev/null\n+++ b/src/new.ts\n@@ -0,0 +1 @@\n+new file",
          },
        ],
      },
    ];

    const summary = buildLatestFileChangeSummary(items);

    expect(summary).toMatchObject({
      files: [{ path: "src/new.ts", status: "A" }],
    });
  });

  it("builds a file summary from a turn-level diff when no fileChange items exist", () => {
    const summary = buildFileChangeSummaryFromTurnDiff(
      [
        "diff --git a/src/a.ts b/src/a.ts",
        "--- a/src/a.ts",
        "+++ b/src/a.ts",
        "@@ -1 +1 @@",
        "-old",
        "+new",
        "diff --git a/src/b.ts b/src/b.ts",
        "new file mode 100644",
        "--- /dev/null",
        "+++ b/src/b.ts",
        "@@ -0,0 +1 @@",
        "+added",
      ].join("\n"),
    );

    expect(summary).toMatchObject({
      files: [
        { path: "src/a.ts", status: "M" },
        { path: "src/b.ts", status: "A" },
      ],
      counts: {
        modified: 1,
        added: 1,
      },
    });
  });
});
