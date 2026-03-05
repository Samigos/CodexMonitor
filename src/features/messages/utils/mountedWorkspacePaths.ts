import { joinWorkspacePath } from "../../../utils/platformPaths";

const WORKSPACE_MOUNT_PREFIX = "/workspace/";
const WORKSPACES_MOUNT_PREFIX = "/workspaces/";

function normalizePathSeparators(path: string) {
  return path.replace(/\\/g, "/");
}

function trimTrailingSeparators(path: string) {
  return path.replace(/[\\/]+$/, "");
}

function pathBaseName(path: string) {
  return trimTrailingSeparators(normalizePathSeparators(path.trim()))
    .split("/")
    .filter(Boolean)
    .pop() ?? "";
}

const DOTLESS_WORKSPACE_FILE_NAMES = new Set([
  "LICENSE",
  "README",
  "CHANGELOG",
  "NOTICE",
  "COPYING",
  "Makefile",
  "Dockerfile",
  "Procfile",
  "Gemfile",
]);

function hasLikelyExplicitWorkspaceSegment(
  segment: string,
  hasNestedPath: boolean,
) {
  if (!segment || segment.startsWith(".")) {
    return false;
  }
  // Single-segment dotted mounts are usually workspace-root files like package.json.
  if (!hasNestedPath && segment.includes(".")) {
    return false;
  }
  if (DOTLESS_WORKSPACE_FILE_NAMES.has(segment)) {
    return false;
  }
  return (
    /[A-Z]/.test(segment) ||
    /[_-]/.test(segment) ||
    (hasNestedPath && segment.includes("."))
  );
}

export function resolveMountedWorkspacePath(
  path: string,
  workspacePath?: string | null,
) {
  const trimmed = path.trim();
  const trimmedWorkspace = workspacePath?.trim() ?? "";
  if (!trimmedWorkspace) {
    return null;
  }

  const normalizedPath = normalizePathSeparators(trimmed);
  const workspaceName = pathBaseName(trimmedWorkspace);
  if (!workspaceName) {
    return null;
  }

  const resolveFromWorkspaceSegments = (
    segments: string[],
    fallbackAbsolutePath?: string,
  ) => {
    if (segments.length === 0) {
      return trimTrailingSeparators(trimmedWorkspace);
    }
    const [firstSegment = "", ...relativeSegments] = segments;
    if (firstSegment === workspaceName) {
      const relativePath = relativeSegments.join("/");
      return relativePath
        ? joinWorkspacePath(trimmedWorkspace, relativePath)
        : trimTrailingSeparators(trimmedWorkspace);
    }
    if (
      fallbackAbsolutePath &&
      hasLikelyExplicitWorkspaceSegment(firstSegment, relativeSegments.length > 0)
    ) {
      // Preserve absolute sibling-workspace mounts instead of rebasing them locally.
      return fallbackAbsolutePath;
    }
    return joinWorkspacePath(trimmedWorkspace, segments.join("/"));
  };

  const resolveFromWorkspacesSegments = (segments: string[]) => {
    if (segments.length === 0) {
      return trimTrailingSeparators(trimmedWorkspace);
    }
    const workspaceIndex = segments.findIndex((segment) => segment === workspaceName);
    if (workspaceIndex < 0) {
      return null;
    }
    const relativePath = segments.slice(workspaceIndex + 1).join("/");
    return relativePath
      ? joinWorkspacePath(trimmedWorkspace, relativePath)
      : trimTrailingSeparators(trimmedWorkspace);
  };

  if (normalizedPath.startsWith(WORKSPACE_MOUNT_PREFIX)) {
    return resolveFromWorkspaceSegments(
      normalizedPath.slice(WORKSPACE_MOUNT_PREFIX.length).split("/").filter(Boolean),
      normalizedPath,
    );
  }
  if (normalizedPath.startsWith(WORKSPACES_MOUNT_PREFIX)) {
    return resolveFromWorkspacesSegments(
      normalizedPath
        .slice(WORKSPACES_MOUNT_PREFIX.length)
        .split("/")
        .filter(Boolean),
    );
  }
  return null;
}
