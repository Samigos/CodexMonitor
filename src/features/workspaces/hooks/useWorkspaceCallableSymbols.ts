import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  DebugEntry,
  WorkspaceCallableSymbol,
  WorkspaceInfo,
} from "../../../types";
import { getWorkspaceCallableSymbols } from "../../../services/tauri";

type UseWorkspaceCallableSymbolsOptions = {
  activeWorkspace: WorkspaceInfo | null;
  onDebug?: (entry: DebugEntry) => void;
  enabled?: boolean;
  pollingEnabled?: boolean;
};

function areSymbolsEqual(a: WorkspaceCallableSymbol[], b: WorkspaceCallableSymbol[]) {
  if (a === b) {
    return true;
  }
  if (a.length !== b.length) {
    return false;
  }
  for (let index = 0; index < a.length; index += 1) {
    const left = a[index];
    const right = b[index];
    if (
      left?.path !== right?.path ||
      left?.symbol !== right?.symbol ||
      left?.kind !== right?.kind ||
      left?.language !== right?.language
    ) {
      return false;
    }
  }
  return true;
}

export function useWorkspaceCallableSymbols({
  activeWorkspace,
  onDebug,
  enabled = true,
  pollingEnabled,
}: UseWorkspaceCallableSymbolsOptions) {
  const [callables, setCallables] = useState<WorkspaceCallableSymbol[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [isDocumentVisible, setIsDocumentVisible] = useState(
    () => document.visibilityState !== "hidden",
  );
  const lastFetchedWorkspaceId = useRef<string | null>(null);
  const inFlight = useRef<string | null>(null);

  const REFRESH_INTERVAL_MS = 60000;
  const LARGE_REFRESH_INTERVAL_MS = 120000;
  const LARGE_SYMBOL_COUNT = 10000;
  const workspaceId = activeWorkspace?.id ?? null;
  const isConnected = Boolean(activeWorkspace?.connected);
  const isEnabled = enabled;
  const isPollingEnabled = pollingEnabled ?? isEnabled;

  const refreshCallables = useCallback(async () => {
    if (!workspaceId || !isConnected || !isEnabled) {
      return;
    }
    if (inFlight.current === workspaceId) {
      return;
    }
    inFlight.current = workspaceId;
    const requestWorkspaceId = workspaceId;
    setIsLoading(true);
    onDebug?.({
      id: `${Date.now()}-client-callables-list`,
      timestamp: Date.now(),
      source: "client",
      label: "callables/list",
      payload: { workspaceId: requestWorkspaceId },
    });
    try {
      const response = await getWorkspaceCallableSymbols(requestWorkspaceId);
      onDebug?.({
        id: `${Date.now()}-server-callables-list`,
        timestamp: Date.now(),
        source: "server",
        label: "callables/list response",
        payload: response,
      });
      if (requestWorkspaceId === workspaceId) {
        const nextCallables = Array.isArray(response) ? response : [];
        setCallables((prev) => (areSymbolsEqual(prev, nextCallables) ? prev : nextCallables));
        lastFetchedWorkspaceId.current = requestWorkspaceId;
      }
    } catch (error) {
      onDebug?.({
        id: `${Date.now()}-client-callables-list-error`,
        timestamp: Date.now(),
        source: "error",
        label: "callables/list error",
        payload: error instanceof Error ? error.message : String(error),
      });
    } finally {
      if (inFlight.current === requestWorkspaceId) {
        inFlight.current = null;
        setIsLoading(false);
      }
    }
  }, [isConnected, isEnabled, onDebug, workspaceId]);

  useEffect(() => {
    setCallables([]);
    lastFetchedWorkspaceId.current = null;
    inFlight.current = null;
  }, [isConnected, workspaceId]);

  useEffect(() => {
    setIsLoading(Boolean(workspaceId && isConnected && isEnabled));
  }, [isConnected, isEnabled, workspaceId]);

  useEffect(() => {
    const handleVisibilityChange = () => {
      setIsDocumentVisible(document.visibilityState !== "hidden");
    };
    document.addEventListener("visibilitychange", handleVisibilityChange);
    return () => {
      document.removeEventListener("visibilitychange", handleVisibilityChange);
    };
  }, []);

  useEffect(() => {
    if (!workspaceId || !isConnected || !isEnabled) {
      return;
    }
    if (lastFetchedWorkspaceId.current === workspaceId && callables.length > 0) {
      return;
    }
    refreshCallables();
  }, [callables.length, isConnected, isEnabled, refreshCallables, workspaceId]);

  useEffect(() => {
    if (!workspaceId || !isConnected || !isPollingEnabled || !isDocumentVisible) {
      return;
    }
    const refreshInterval =
      callables.length > LARGE_SYMBOL_COUNT ? LARGE_REFRESH_INTERVAL_MS : REFRESH_INTERVAL_MS;

    const interval = window.setInterval(() => {
      if (document.visibilityState === "hidden") {
        return;
      }
      refreshCallables().catch(() => {});
    }, refreshInterval);

    return () => {
      window.clearInterval(interval);
    };
  }, [
    callables.length,
    isConnected,
    isDocumentVisible,
    isPollingEnabled,
    refreshCallables,
    workspaceId,
  ]);

  const callableOptions = useMemo(
    () => callables.filter((entry) => Boolean(entry?.path) && Boolean(entry?.symbol)),
    [callables],
  );

  return {
    callables: callableOptions,
    isLoading,
    refreshCallables,
  };
}
