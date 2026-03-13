/** @vitest-environment jsdom */
import { createRef } from "react";
import { renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { WorkspaceCallableSymbol } from "../../../types";
import { useComposerAutocompleteState } from "./useComposerAutocompleteState";

describe("useComposerAutocompleteState file mentions", () => {
  it("suggests a file even if it is already mentioned earlier in the message", () => {
    const files = ["src/App.tsx", "src/main.tsx"];
    const text = "Please review @src/App.tsx and also @";
    const selectionStart = text.length;
    const textareaRef = createRef<HTMLTextAreaElement>();
    textareaRef.current = {
      focus: vi.fn(),
      setSelectionRange: vi.fn(),
    } as unknown as HTMLTextAreaElement;

    const { result } = renderHook(() =>
      useComposerAutocompleteState({
        text,
        selectionStart,
        disabled: false,
        appsEnabled: true,
        skills: [],
        apps: [],
        prompts: [],
        files,
        callables: [],
        textareaRef,
        setText: vi.fn(),
        setSelectionStart: vi.fn(),
      }),
    );

    expect(result.current.isAutocompleteOpen).toBe(true);
    expect(result.current.autocompleteMatches.map((item) => item.label)).toContain(
      "src/App.tsx",
    );
  });

  it("marks root-level file suggestions as Files group", () => {
    const files = ["AGENTS.md", "src/main.tsx"];
    const text = "@";
    const selectionStart = text.length;
    const textareaRef = createRef<HTMLTextAreaElement>();
    textareaRef.current = {
      focus: vi.fn(),
      setSelectionRange: vi.fn(),
    } as unknown as HTMLTextAreaElement;

    const { result } = renderHook(() =>
      useComposerAutocompleteState({
        text,
        selectionStart,
        disabled: false,
        appsEnabled: true,
        skills: [],
        apps: [],
        prompts: [],
        files,
        callables: [],
        textareaRef,
        setText: vi.fn(),
        setSelectionStart: vi.fn(),
      }),
    );

    const rootItem = result.current.autocompleteMatches.find(
      (item) => item.label === "AGENTS.md",
    );
    expect(rootItem?.group).toBe("Files");
  });
});

describe("useComposerAutocompleteState slash commands", () => {
  it("includes built-in slash commands in alphabetical order when apps are enabled", () => {
    const text = "/";
    const selectionStart = text.length;
    const textareaRef = createRef<HTMLTextAreaElement>();
    textareaRef.current = {
      focus: vi.fn(),
      setSelectionRange: vi.fn(),
    } as unknown as HTMLTextAreaElement;

    const { result } = renderHook(() =>
      useComposerAutocompleteState({
        text,
        selectionStart,
        disabled: false,
        appsEnabled: true,
        skills: [],
        apps: [],
        prompts: [],
        files: [],
        callables: [],
        textareaRef,
        setText: vi.fn(),
        setSelectionStart: vi.fn(),
      }),
    );

    const labels = result.current.autocompleteMatches.map((item) => item.label);
    expect(labels).toEqual(
      expect.arrayContaining([
        "apps",
        "compact",
        "fast",
        "fork",
        "mcp",
        "new",
        "resume",
        "review",
        "status",
      ]),
    );
    expect(labels.slice(0, 9)).toEqual([
      "apps",
      "compact",
      "fast",
      "fork",
      "mcp",
      "new",
      "resume",
      "review",
      "status",
    ]);
  });

  it("hides /apps when apps are disabled", () => {
    const text = "/";
    const selectionStart = text.length;
    const textareaRef = createRef<HTMLTextAreaElement>();
    textareaRef.current = {
      focus: vi.fn(),
      setSelectionRange: vi.fn(),
    } as unknown as HTMLTextAreaElement;

    const { result } = renderHook(() =>
      useComposerAutocompleteState({
        text,
        selectionStart,
        disabled: false,
        appsEnabled: false,
        skills: [],
        apps: [],
        prompts: [],
        files: [],
        callables: [],
        textareaRef,
        setText: vi.fn(),
        setSelectionStart: vi.fn(),
      }),
    );

    const labels = result.current.autocompleteMatches.map((item) => item.label);
    expect(labels).not.toContain("apps");
    expect(labels).toEqual([
      "compact",
      "fast",
      "fork",
      "mcp",
      "new",
      "resume",
      "review",
      "status",
    ]);
  });
});

describe("useComposerAutocompleteState $ completions", () => {
  it("separates skills and apps into grouped results", () => {
    const text = "$";
    const selectionStart = text.length;
    const textareaRef = createRef<HTMLTextAreaElement>();
    textareaRef.current = {
      focus: vi.fn(),
      setSelectionRange: vi.fn(),
    } as unknown as HTMLTextAreaElement;

    const { result } = renderHook(() =>
      useComposerAutocompleteState({
        text,
        selectionStart,
        disabled: false,
        appsEnabled: true,
        skills: [
          { name: "skill-a", description: "Skill A" },
          { name: "skill-b", description: "Skill B" },
        ],
        apps: [
          {
            id: "connector_calendar",
            name: "Calendar App",
            description: "Calendar app",
            isAccessible: true,
            installUrl: null,
            distributionChannel: null,
          },
          {
            id: "not-ready",
            name: "Not Ready App",
            description: "Unreleased",
            isAccessible: false,
            installUrl: "https://example.com/install",
            distributionChannel: "beta",
          },
        ],
        prompts: [],
        files: [],
        callables: [],
        textareaRef,
        setText: vi.fn(),
        setSelectionStart: vi.fn(),
      }),
    );

    const ids = result.current.autocompleteMatches.map((item) => item.id);
    const groups = result.current.autocompleteMatches.map((item) => item.group);
    const appSuggestion = result.current.autocompleteMatches.find(
      (item) => item.id === "app:connector_calendar",
    );
    expect(ids).toEqual(["skill:skill-a", "skill:skill-b", "app:connector_calendar"]);
    expect(groups).toEqual(["Skills", "Skills", "Apps"]);
    expect(ids).not.toContain("app:not-ready");
    expect(appSuggestion?.insertText).toBe("calendar-app");
    expect(appSuggestion?.mentionPath).toBe("app://connector_calendar");
  });

  it("shows grouped function suggestions for specific @ queries", () => {
    const text = "@use";
    const selectionStart = text.length;
    const textareaRef = createRef<HTMLTextAreaElement>();
    textareaRef.current = {
      focus: vi.fn(),
      setSelectionRange: vi.fn(),
    } as unknown as HTMLTextAreaElement;

    const { result } = renderHook(() =>
      useComposerAutocompleteState({
        text,
        selectionStart,
        disabled: false,
        appsEnabled: true,
        skills: [],
        apps: [],
        prompts: [],
        files: ["src/hooks/useTheme.ts"],
        callables: [
          {
            path: "src/hooks/useComposerAutocompleteState.ts",
            symbol: "useComposerAutocompleteState",
            kind: "hook",
            language: "typescript",
          },
          {
            path: "src/hooks/useTheme.ts",
            symbol: "useTheme",
            kind: "hook",
            language: "typescript",
          },
        ],
        textareaRef,
        setText: vi.fn(),
        setSelectionStart: vi.fn(),
      }),
    );

    expect(result.current.autocompleteMatches.map((item) => item.group)).toEqual([
      "Functions",
      "Functions",
      "Files",
    ]);
    expect(result.current.autocompleteMatches[0]).toMatchObject({
      label: "useTheme",
      description: "src/hooks/useTheme.ts",
      insertText: "src/hooks/useTheme.ts#useTheme",
    });
    expect(result.current.autocompleteMatches[1]).toMatchObject({
      label: "useComposerAutocompleteState",
      description: "src/hooks/useComposerAutocompleteState.ts",
      insertText: "src/hooks/useComposerAutocompleteState.ts#useComposerAutocompleteState",
    });
    expect(result.current.autocompleteMatches[2]).toMatchObject({
      label: "src/hooks/useTheme.ts",
      insertText: "src/hooks/useTheme.ts",
    });
  });

  it("does not show function suggestions on an empty @ query", () => {
    const text = "@";
    const selectionStart = text.length;
    const textareaRef = createRef<HTMLTextAreaElement>();
    textareaRef.current = {
      focus: vi.fn(),
      setSelectionRange: vi.fn(),
    } as unknown as HTMLTextAreaElement;

    const { result } = renderHook(() =>
      useComposerAutocompleteState({
        text,
        selectionStart,
        disabled: false,
        appsEnabled: true,
        skills: [],
        apps: [],
        prompts: [],
        files: ["src/main.tsx"],
        callables: [
          {
            path: "src/main.tsx",
            symbol: "App",
            kind: "component",
            language: "typescript",
          },
        ],
        textareaRef,
        setText: vi.fn(),
        setSelectionStart: vi.fn(),
      }),
    );

    expect(result.current.autocompleteMatches.map((item) => item.group)).toEqual(["Files"]);
  });

  it("does not match functions by fuzzy subsequence on the containing file path", () => {
    const text = "@hmvw";
    const selectionStart = text.length;
    const textareaRef = createRef<HTMLTextAreaElement>();
    textareaRef.current = {
      focus: vi.fn(),
      setSelectionRange: vi.fn(),
    } as unknown as HTMLTextAreaElement;

    const { result } = renderHook(() =>
      useComposerAutocompleteState({
        text,
        selectionStart,
        disabled: false,
        appsEnabled: true,
        skills: [],
        apps: [],
        prompts: [],
        files: ["src/views/HomeView.tsx"],
        callables: [
          {
            path: "src/views/HomeView.tsx",
            symbol: "renderPanel",
            kind: "function",
            language: "typescript",
          },
          {
            path: "src/views/HomeView.tsx",
            symbol: "bindKeyboardShortcuts",
            kind: "function",
            language: "typescript",
          },
        ],
        textareaRef,
        setText: vi.fn(),
        setSelectionStart: vi.fn(),
      }),
    );

    expect(result.current.autocompleteMatches).toEqual([
      expect.objectContaining({
        group: "Files",
        label: "src/views/HomeView.tsx",
      }),
    ]);
  });

  it("caps @ results to 20 functions and 50 files separately", () => {
    const text = "@src";
    const selectionStart = text.length;
    const textareaRef = createRef<HTMLTextAreaElement>();
    textareaRef.current = {
      focus: vi.fn(),
      setSelectionRange: vi.fn(),
    } as unknown as HTMLTextAreaElement;

    const files = Array.from({ length: 80 }, (_, index) => `src/features/file-${index}.ts`);
    const callables: WorkspaceCallableSymbol[] = Array.from({ length: 30 }, (_, index) => ({
      path: `src/features/file-${index}.ts`,
      symbol: `useFeature${index}`,
      kind: "hook",
      language: "typescript",
    }));

    const { result } = renderHook(() =>
      useComposerAutocompleteState({
        text,
        selectionStart,
        disabled: false,
        appsEnabled: true,
        skills: [],
        apps: [],
        prompts: [],
        files,
        callables,
        textareaRef,
        setText: vi.fn(),
        setSelectionStart: vi.fn(),
      }),
    );

    expect(result.current.autocompleteMatches).toHaveLength(70);
    expect(
      result.current.autocompleteMatches.filter((item) => item.group === "Functions"),
    ).toHaveLength(20);
    expect(
      result.current.autocompleteMatches.filter((item) => item.group === "Files"),
    ).toHaveLength(50);
  });

  it("caps empty @ queries to 50 file results", () => {
    const text = "@";
    const selectionStart = text.length;
    const textareaRef = createRef<HTMLTextAreaElement>();
    textareaRef.current = {
      focus: vi.fn(),
      setSelectionRange: vi.fn(),
    } as unknown as HTMLTextAreaElement;

    const files = Array.from({ length: 80 }, (_, index) => `src/features/file-${index}.ts`);

    const { result } = renderHook(() =>
      useComposerAutocompleteState({
        text,
        selectionStart,
        disabled: false,
        appsEnabled: true,
        skills: [],
        apps: [],
        prompts: [],
        files,
        callables: [],
        textareaRef,
        setText: vi.fn(),
        setSelectionStart: vi.fn(),
      }),
    );

    expect(result.current.autocompleteMatches).toHaveLength(50);
    expect(result.current.autocompleteMatches.every((item) => item.group === "Files")).toBe(
      true,
    );
  });
});
