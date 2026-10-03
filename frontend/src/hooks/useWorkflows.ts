"use client";

import { useDashboard } from "@/hooks/useDashboard";

/** Workflows are included in the dashboard WebSocket snapshot. */
export function useWorkflows() {
  const { workflows, loading } = useDashboard();
  return { workflows, loading };
}
