import { useCallback, useEffect, useState } from "react";
import { relayApi } from "../lib/relayApi";
import { appendTaskLogChunk, clearTaskLogBuffer, type TaskLogBuffer } from "../lib/taskLogStream";

export function useTaskLogStream() {
  const [logsByTask, setLogsByTask] = useState<Record<string, TaskLogBuffer>>({});

  useEffect(() => {
    let active = true;
    let unsub: (() => void) | undefined;

    void relayApi
      .subscribeTaskLogEvents((event) => {
        if (!active) return;
        setLogsByTask((prev) => appendTaskLogChunk(prev, event));
      })
      .then((fn) => {
        if (active) {
          unsub = fn;
        } else {
          fn();
        }
      });

    return () => {
      active = false;
      unsub?.();
    };
  }, []);

  const clearTaskLogs = useCallback((taskId?: string) => {
    setLogsByTask((prev) => clearTaskLogBuffer(prev, taskId));
  }, []);

  return {
    logsByTask,
    clearTaskLogs,
  };
}
