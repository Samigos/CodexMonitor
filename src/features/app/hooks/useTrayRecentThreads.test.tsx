// @vitest-environment jsdom
import { renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ThreadSummary, WorkspaceInfo } from "../../../types";
import {
  buildTrayRecentThreadEntries,
  useTrayRecentThreads,
} from "./useTrayRecentThreads";

const setTrayRecentThreadsMock = vi.fn();

vi.mock("@services/tauri", () => ({
  setTrayRecentThreads: (...args: unknown[]) => setTrayRecentThreadsMock(...args),
}));

function makeWorkspace(overrides: Partial<WorkspaceInfo> = {}): WorkspaceInfo {
  return {
    id: "ws-1",
    name: "Workspace One",
    path: "/tmp/workspace-one",
    connected: true,
    settings: { sidebarCollapsed: false },
    ...overrides,
  };
}

function makeThread(overrides: Partial<ThreadSummary> = {}): ThreadSummary {
  return {
    id: "thread-1",
    name: "Thread One",
    updatedAt: 1,
    ...overrides,
  };
}

describe("useTrayRecentThreads", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    setTrayRecentThreadsMock.mockReset();
    setTrayRecentThreadsMock.mockResolvedValue(undefined);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("builds global recents ordered by updatedAt and excludes subagents", () => {
    const workspaces = [
      makeWorkspace(),
      makeWorkspace({
        id: "ws-2",
        name: "Workspace Two",
        path: "/tmp/workspace-two",
      }),
    ];
    const threadsByWorkspace = {
      "ws-1": [
        makeThread({ id: "thread-1", name: "Alpha", updatedAt: 10 }),
        makeThread({ id: "thread-2", name: "Beta", updatedAt: 30 }),
      ],
      "ws-2": [
        makeThread({ id: "thread-3", name: "Alpha", updatedAt: 20 }),
        makeThread({ id: "thread-4", name: "Hidden", updatedAt: 40 }),
      ],
    };

    const entries = buildTrayRecentThreadEntries(
      workspaces,
      threadsByWorkspace,
      (workspaceId, threadId) => workspaceId === "ws-2" && threadId === "thread-4",
    );

    expect(entries).toEqual([
      {
        workspaceId: "ws-1",
        workspaceLabel: "Workspace One",
        threadId: "thread-2",
        threadLabel: "Workspace One: Beta",
        updatedAt: 30,
      },
      {
        workspaceId: "ws-2",
        workspaceLabel: "Workspace Two",
        threadId: "thread-3",
        threadLabel: "Workspace Two: Alpha",
        updatedAt: 20,
      },
      {
        workspaceId: "ws-1",
        workspaceLabel: "Workspace One",
        threadId: "thread-1",
        threadLabel: "Workspace One: Alpha",
        updatedAt: 10,
      },
    ]);
  });

  it("syncs only when the derived recent list changes", async () => {
    const workspaces = [makeWorkspace()];
    const initialThreads = {
      "ws-1": [makeThread({ id: "thread-1", name: "Alpha", updatedAt: 10 })],
    };

    const { rerender } = renderHook(
      ({
        threadsByWorkspace,
      }: {
        threadsByWorkspace: Record<string, ThreadSummary[]>;
      }) =>
        useTrayRecentThreads({
          workspaces,
          threadsByWorkspace,
          isSubagentThread: () => false,
        }),
      {
        initialProps: {
          threadsByWorkspace: initialThreads,
        },
      },
    );

    await vi.runAllTimersAsync();
    expect(setTrayRecentThreadsMock).toHaveBeenCalledTimes(1);

    rerender({ threadsByWorkspace: initialThreads });
    await vi.runAllTimersAsync();
    expect(setTrayRecentThreadsMock).toHaveBeenCalledTimes(1);

    rerender({
      threadsByWorkspace: {
        "ws-1": [makeThread({ id: "thread-1", name: "Alpha", updatedAt: 20 })],
      },
    });
    await vi.runAllTimersAsync();

    expect(setTrayRecentThreadsMock).toHaveBeenCalledTimes(2);
    expect(setTrayRecentThreadsMock).toHaveBeenLastCalledWith([
      {
        workspaceId: "ws-1",
        workspaceLabel: "Workspace One",
        threadId: "thread-1",
        threadLabel: "Workspace One: Alpha",
        updatedAt: 20,
      },
    ]);
  });
});
