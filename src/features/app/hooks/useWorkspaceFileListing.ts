import { useEffect, useState } from "react";
import type { DebugEntry, WorkspaceCallableSymbol, WorkspaceInfo } from "../../../types";
import { useWorkspaceFiles } from "../../workspaces/hooks/useWorkspaceFiles";
import { useWorkspaceCallableSymbols } from "../../workspaces/hooks/useWorkspaceCallableSymbols";

type FilePanelMode = "git" | "files" | "prompts";
type TabKey = "home" | "projects" | "codex" | "git" | "log";
type TabletTabKey = "codex" | "git" | "log";

type UseWorkspaceFileListingArgs = {
  activeWorkspace: WorkspaceInfo | null;
  activeWorkspaceId: string | null;
  filePanelMode: FilePanelMode;
  isCompact: boolean;
  isTablet: boolean;
  activeTab: TabKey;
  tabletTab: TabletTabKey;
  rightPanelCollapsed: boolean;
  hasComposerSurface: boolean;
  onDebug?: (entry: DebugEntry) => void;
};

type UseWorkspaceFileListingResult = {
  files: string[];
  callables: WorkspaceCallableSymbol[];
  isLoading: boolean;
  setProjectAutocompleteActive: (active: boolean) => void;
};

export function useWorkspaceFileListing({
  activeWorkspace,
  activeWorkspaceId,
  filePanelMode,
  isCompact,
  isTablet,
  activeTab,
  tabletTab,
  rightPanelCollapsed,
  hasComposerSurface,
  onDebug,
}: UseWorkspaceFileListingArgs): UseWorkspaceFileListingResult {
  const [projectAutocompleteActive, setProjectAutocompleteActive] = useState(false);

  const compactTab = isTablet ? tabletTab : activeTab;
  const filePanelVisible =
    filePanelMode === "files" &&
    (isCompact ? compactTab === "git" : !rightPanelCollapsed);
  const shouldFetchFiles =
    Boolean(activeWorkspace) && (filePanelMode === "files" || projectAutocompleteActive);
  const shouldFetchCallables = Boolean(activeWorkspace) && projectAutocompleteActive;

  useEffect(() => {
    if (!activeWorkspaceId) {
      setProjectAutocompleteActive(false);
    }
  }, [activeWorkspaceId]);

  useEffect(() => {
    if (!hasComposerSurface) {
      setProjectAutocompleteActive(false);
    }
  }, [hasComposerSurface]);

  const { files, isLoading } = useWorkspaceFiles({
    activeWorkspace,
    onDebug,
    enabled: shouldFetchFiles,
    pollingEnabled: filePanelVisible,
  });

  const { callables, isLoading: isCallablesLoading } = useWorkspaceCallableSymbols({
    activeWorkspace,
    onDebug,
    enabled: shouldFetchCallables,
    pollingEnabled: projectAutocompleteActive,
  });

  return {
    files,
    callables,
    isLoading: isLoading || isCallablesLoading,
    setProjectAutocompleteActive,
  };
}
