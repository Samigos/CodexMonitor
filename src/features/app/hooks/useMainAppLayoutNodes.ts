import { useLayoutNodes } from "@/features/layout/hooks/useLayoutNodes";
import type { LayoutNodesOptions } from "@/features/layout/hooks/layoutNodes/types";

export function useMainAppLayoutNodes(options: LayoutNodesOptions) {
  return useLayoutNodes(options);
}
