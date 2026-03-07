import { useEffect, useMemo, useState } from "react";

export type AutocompleteItem = {
  id: string;
  label: string;
  description?: string;
  insertText?: string;
  searchText?: string;
  hint?: string;
  cursorOffset?: number;
  group?: "Files" | "Functions" | "Skills" | "Apps" | "Slash" | "Prompts";
  mentionPath?: string;
};

export type AutocompleteTrigger = {
  trigger: string;
  items: AutocompleteItem[];
};

type AutocompleteRange = {
  start: number;
  end: number;
};

type AutocompleteState = {
  active: boolean;
  trigger: string | null;
  query: string;
  range: AutocompleteRange | null;
};

type UseComposerAutocompleteArgs = {
  text: string;
  selectionStart: number | null;
  triggers: AutocompleteTrigger[];
  maxResults?: number;
};

const whitespaceRegex = /\s/;
const triggerPrefixRegex = /^(?:\s|["'`]|\(|\[|\{)$/;

function resolveAutocompleteState(
  text: string,
  cursor: number,
  triggers: AutocompleteTrigger[],
): AutocompleteState {
  if (cursor <= 0) {
    return { active: false, trigger: null, query: "", range: null };
  }
  const triggerSet = new Set(triggers.map((entry) => entry.trigger));
  let index = cursor - 1;
  while (index >= 0) {
    const char = text[index];
    if (whitespaceRegex.test(char)) {
      break;
    }
    if (triggerSet.has(char)) {
      const prevChar = index > 0 ? text[index - 1] : "";
      if (!prevChar || triggerPrefixRegex.test(prevChar)) {
        const query = text.slice(index + 1, cursor);
        return {
          active: true,
          trigger: char,
          query,
          range: { start: index + 1, end: cursor },
        };
      }
    }
    index -= 1;
  }
  return { active: false, trigger: null, query: "", range: null };
}

function basename(label: string) {
  const normalized = label.replace(/\\/g, "/");
  const parts = normalized.split("/").filter(Boolean);
  return parts.length ? parts[parts.length - 1] : label;
}

function fileParts(label: string) {
  const normalized = label.replace(/\\/g, "/").toLowerCase();
  const base = basename(normalized);
  const dotIndex = base.lastIndexOf(".");
  const name =
    dotIndex > 0 && dotIndex < base.length - 1 ? base.slice(0, dotIndex) : base;
  const ext =
    dotIndex > 0 && dotIndex < base.length - 1 ? base.slice(dotIndex + 1) : "";
  return { normalized, base, name, ext };
}

function isSubsequence(query: string, target: string) {
  let q = 0;
  let t = 0;
  while (q < query.length && t < target.length) {
    if (query[q] === target[t]) {
      q += 1;
    }
    t += 1;
  }
  return q === query.length;
}

function scoreFileMatch(query: string, label: string) {
  if (!query) {
    return 0;
  }
  const normalizedQuery = query.toLowerCase();
  const { normalized, base, name, ext } = fileParts(label);
  const queryParts = normalizedQuery.split(".");
  const queryName = queryParts[0] ?? "";
  const queryExt = queryParts.length > 1 ? queryParts.slice(1).join(".") : "";
  const matchExt =
    !queryExt || ext.startsWith(queryExt) || ext.includes(queryExt);
  if (!matchExt) {
    return 0;
  }

  if (!queryName) {
    if (queryExt && ext === queryExt) {
      return 60;
    }
    if (queryExt) {
      return 40;
    }
    return 0;
  }

  if (normalized === normalizedQuery || name === queryName) {
    return 110;
  }
  if (name.startsWith(queryName)) {
    return 95 + (queryExt ? 10 : 0);
  }
  if (base.startsWith(queryName)) {
    return 90 + (queryExt ? 10 : 0);
  }
  if (normalized.startsWith(queryName)) {
    return 80 + (queryExt ? 5 : 0);
  }
  if (name.includes(queryName)) {
    return 70 + (queryExt ? 5 : 0);
  }
  if (normalized.includes(queryName)) {
    return 60 + (queryExt ? 5 : 0);
  }
  if (isSubsequence(queryName, name)) {
    return 50 + (queryExt ? 5 : 0);
  }
  return 0;
}

function scoreTextMatch(query: string, target: string) {
  if (!query || !target) {
    return 0;
  }
  const normalizedTarget = target.toLowerCase();
  if (normalizedTarget === query) {
    return 120;
  }
  if (normalizedTarget.startsWith(query)) {
    return 100;
  }
  if (normalizedTarget.includes(query)) {
    return 80;
  }
  if (isSubsequence(query, normalizedTarget)) {
    return 55;
  }
  return 0;
}

function scoreFunctionMatch(query: string, item: AutocompleteItem) {
  const symbol = item.label.toLowerCase();
  const path = (item.description ?? "").replace(/\\/g, "/").toLowerCase();
  const combined = (item.searchText ?? `${path}#${symbol}`).toLowerCase();

  return Math.max(
    scoreTextMatch(query, symbol) + 25,
    scoreTextMatch(query, combined) + 10,
    scoreTextMatch(query, path),
  );
}

function scoreItem(query: string, item: AutocompleteItem) {
  if (item.group === "Files") {
    return scoreFileMatch(query, item.label);
  }
  if (item.group === "Functions") {
    return scoreFunctionMatch(query, item);
  }
  return Math.max(
    scoreTextMatch(query, item.label),
    scoreTextMatch(query, item.searchText ?? ""),
  );
}

function groupPriority(group: AutocompleteItem["group"]) {
  switch (group) {
    case "Functions":
      return 0;
    case "Files":
      return 1;
    case "Skills":
      return 0;
    case "Apps":
      return 1;
    case "Slash":
      return 0;
    case "Prompts":
      return 1;
    default:
      return 99;
  }
}

function rankItems(items: AutocompleteItem[], query: string) {
  const normalized = query.trim().toLowerCase();
  if (!normalized) {
    return items.slice();
  }
  const ranked = items
    .map((item) => ({
      item,
      score: scoreItem(normalized, item),
    }))
    .filter((entry) => entry.score > 0)
    .sort((a, b) => {
      const groupDelta = groupPriority(a.item.group) - groupPriority(b.item.group);
      if (groupDelta !== 0) {
        return groupDelta;
      }
      if (a.score !== b.score) {
        return b.score - a.score;
      }
      if (a.item.label.length !== b.item.label.length) {
        return a.item.label.length - b.item.label.length;
      }
      return a.item.label.localeCompare(b.item.label);
    });
  return ranked.map((entry) => entry.item);
}

export function useComposerAutocomplete({
  text,
  selectionStart,
  triggers,
  maxResults = 50,
}: UseComposerAutocompleteArgs) {
  const [highlightIndex, setHighlightIndex] = useState(0);
  const [dismissed, setDismissed] = useState(false);

  const state = useMemo(() => {
    if (selectionStart === null || selectionStart < 0) {
      return { active: false, trigger: null, query: "", range: null };
    }
    return resolveAutocompleteState(text, selectionStart, triggers);
  }, [selectionStart, text, triggers]);

  const matches = useMemo(() => {
    if (!state.active || !state.trigger) {
      return [];
    }
    const source = triggers.find((entry) => entry.trigger === state.trigger);
    if (!source) {
      return [];
    }
    const ranked = rankItems(source.items, state.query);
    return ranked.slice(0, Math.max(0, maxResults));
  }, [state.active, state.query, state.trigger, triggers, maxResults]);

  useEffect(() => {
    setHighlightIndex(0);
    setDismissed(false);
  }, [state.active, state.query, state.trigger, state.range?.start, state.range?.end]);

  const moveHighlight = (delta: number) => {
    if (matches.length === 0) {
      return;
    }
    setHighlightIndex((prev) => {
      const next = (prev + delta + matches.length) % matches.length;
      return next;
    });
  };

  const close = () => {
    setHighlightIndex(0);
    setDismissed(true);
  };

  return {
    active: state.active && matches.length > 0 && !dismissed,
    query: state.query,
    range: state.range,
    matches,
    highlightIndex,
    setHighlightIndex,
    moveHighlight,
    close,
  };
}
