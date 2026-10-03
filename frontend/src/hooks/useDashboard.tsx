"use client";

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  type AudioLevels,
  type DashboardSnapshot,
  type PlayoutClient,
  type RecordingInfo,
  type SrtInfo,
  type Stream,
  type TcLoopInfo,
  type CommentatorInfo,
} from "@/lib/api";
import { mediaBase } from "@/lib/mediaBase";
import { sortByChannelId } from "@/lib/sortChannels";
import { normalizeWorkflowConfig, type ChannelWorkflowConfig } from "@/lib/workflow";

const API_KEY = process.env.NEXT_PUBLIC_API_KEY ?? "";
const WS_CONNECT_TIMEOUT_MS = 8_000;

export type { DashboardSnapshot };

type DashboardState = {
  connected: boolean;
  loading: boolean;
  streams: Stream[];
  playout: PlayoutClient[];
  tcById: Record<number, TcLoopInfo>;
  commentatorById: Record<number, CommentatorInfo>;
  recordings: Record<number, RecordingInfo>;
  srtById: Record<number, SrtInfo>;
  workflows: Record<number, ChannelWorkflowConfig>;
};

const EMPTY: DashboardState = {
  connected: false,
  loading: true,
  streams: [],
  playout: [],
  tcById: {},
  commentatorById: {},
  recordings: {},
  srtById: {},
  workflows: {},
};

const DashboardContext = createContext<DashboardState>(EMPTY);

export type MeterBus = "encode" | "playout";

type MetersState = {
  encode: Record<number, AudioLevels>;
  playout: Record<number, AudioLevels>;
};

const EMPTY_METERS: MetersState = { encode: {}, playout: {} };
const MetersContext = createContext<MetersState>(EMPTY_METERS);

function wsURL(): string {
  const u = new URL("/ws", mediaBase());
  u.protocol = u.protocol === "https:" ? "wss:" : "ws:";
  u.search = "";
  if (API_KEY) u.searchParams.set("api_key", API_KEY);
  return u.toString();
}

function mapMeters(raw?: Record<string, AudioLevels>): Record<number, AudioLevels> {
  const out: Record<number, AudioLevels> = {};
  if (!raw) return out;
  for (const [k, v] of Object.entries(raw)) {
    const id = Number(k);
    if (Number.isFinite(id) && v) out[id] = v;
  }
  return out;
}

function mapWorkflows(
  raw?: Record<string, Partial<ChannelWorkflowConfig>>,
): Record<number, ChannelWorkflowConfig> {
  const out: Record<number, ChannelWorkflowConfig> = {};
  if (!raw) return out;
  for (const [key, value] of Object.entries(raw)) {
    const id = Number(key);
    if (Number.isFinite(id)) out[id] = normalizeWorkflowConfig(value);
  }
  return out;
}

function snapshotToState(msg: DashboardSnapshot, connected: boolean): DashboardState {
  const rec: Record<number, RecordingInfo> = {};
  for (const r of msg.recordings ?? []) rec[r.id] = r;
  const srt: Record<number, SrtInfo> = {};
  for (const s of msg.srt ?? []) srt[s.id] = s;
  const tc: Record<number, TcLoopInfo> = {};
  for (const t of msg.tc ?? []) tc[t.id] = t;
  const commentator: Record<number, CommentatorInfo> = {};
  for (const c of msg.commentator ?? []) commentator[c.id] = c;
  return {
    connected,
    loading: false,
    streams: sortByChannelId(msg.streams ?? []),
    playout: sortByChannelId(msg.playout ?? []),
    tcById: tc,
    commentatorById: commentator,
    recordings: rec,
    srtById: srt,
    workflows: mapWorkflows(msg.workflows),
  };
}

export function DashboardProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<DashboardState>(EMPTY);
  const [meters, setMeters] = useState<MetersState>(EMPTY_METERS);
  const retryRef = useRef(0);
  const wsRef = useRef<WebSocket | null>(null);
  const retryTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const connectTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const intentionalCloseRef = useRef(false);
  const sessionRef = useRef(0);
  const hadDataRef = useRef(false);
  const connectRef = useRef<(session: number) => void>(() => {});

  const clearRetryTimer = useCallback(() => {
    if (retryTimerRef.current != null) {
      clearTimeout(retryTimerRef.current);
      retryTimerRef.current = null;
    }
  }, []);

  const clearConnectTimer = useCallback(() => {
    if (connectTimerRef.current != null) {
      clearTimeout(connectTimerRef.current);
      connectTimerRef.current = null;
    }
  }, []);

  const applySnapshot = useCallback((msg: DashboardSnapshot, connected: boolean, session: number) => {
    if (session !== sessionRef.current) return;
    hadDataRef.current = true;
    setState(snapshotToState(msg, connected));
  }, []);

  const scheduleReconnect = useCallback(
    (session: number) => {
      const attempt = retryRef.current++;
      const delay = Math.min(10_000, 500 * 2 ** Math.min(attempt, 4));
      retryTimerRef.current = setTimeout(() => {
        retryTimerRef.current = null;
        connectRef.current(session);
      }, delay);
    },
    [],
  );

  const connect = useCallback(
    (session: number) => {
      if (typeof window === "undefined") return;

      clearRetryTimer();
      clearConnectTimer();
      intentionalCloseRef.current = false;

      const existing = wsRef.current;
      if (existing) {
        intentionalCloseRef.current = true;
        try {
          existing.close();
        } catch {
          /* ignore */
        }
        wsRef.current = null;
        intentionalCloseRef.current = false;
      }

      const sock = new WebSocket(wsURL());
      wsRef.current = sock;

      connectTimerRef.current = setTimeout(() => {
        connectTimerRef.current = null;
        if (session !== sessionRef.current || wsRef.current !== sock) return;
        if (sock.readyState !== WebSocket.CONNECTING) return;
        intentionalCloseRef.current = true;
        try {
          sock.close();
        } catch {
          /* ignore */
        }
        intentionalCloseRef.current = false;
        wsRef.current = null;
        if (!hadDataRef.current) {
          setState((prev) => ({ ...prev, loading: false, connected: false }));
        } else {
          setState((prev) => ({ ...prev, connected: false }));
        }
        scheduleReconnect(session);
      }, WS_CONNECT_TIMEOUT_MS);

      sock.onopen = () => {
        if (session !== sessionRef.current || wsRef.current !== sock) return;
        clearConnectTimer();
        retryRef.current = 0;
        setState((prev) => ({ ...prev, connected: true }));
      };

      sock.onmessage = (ev) => {
        if (session !== sessionRef.current || wsRef.current !== sock) return;
        try {
          const msg = JSON.parse(String(ev.data)) as DashboardSnapshot | { type?: string; meters_encode?: Record<string, AudioLevels>; meters_playout?: Record<string, AudioLevels> };
          if (msg?.type === "meters") {
            setMeters({
              encode: mapMeters(msg.meters_encode),
              playout: mapMeters(msg.meters_playout),
            });
            return;
          }
          if (msg?.type !== "snapshot") return;
          const snap = msg as DashboardSnapshot;
          applySnapshot(snap, true, session);
          setMeters({
            encode: mapMeters(snap.meters_encode),
            playout: mapMeters(snap.meters_playout),
          });
        } catch {
          /* ignore bad frames */
        }
      };

      sock.onclose = () => {
        if (session !== sessionRef.current || wsRef.current !== sock) return;
        clearConnectTimer();
        wsRef.current = null;
        if (intentionalCloseRef.current) return;

        setState((prev) => ({ ...prev, connected: false }));
        scheduleReconnect(session);
      };

      sock.onerror = () => {
        sock.close();
      };
    },
    [applySnapshot, clearConnectTimer, clearRetryTimer, scheduleReconnect],
  );

  connectRef.current = connect;

  useEffect(() => {
    const session = ++sessionRef.current;
    connect(session);

    const dropSocket = () => {
      intentionalCloseRef.current = true;
      try {
        wsRef.current?.close();
      } catch {
        /* ignore */
      }
      wsRef.current = null;
    };

    window.addEventListener("pagehide", dropSocket);

    return () => {
      sessionRef.current++;
      clearRetryTimer();
      clearConnectTimer();
      window.removeEventListener("pagehide", dropSocket);
      dropSocket();
    };
  }, [connect, clearRetryTimer, clearConnectTimer]);

  const value = useMemo(() => state, [state]);
  return (
    <DashboardContext.Provider value={value}>
      <MetersContext.Provider value={meters}>{children}</MetersContext.Provider>
    </DashboardContext.Provider>
  );
}

export function useDashboard(): DashboardState {
  return useContext(DashboardContext);
}

export function useMeterLevels(id: number, bus: MeterBus): AudioLevels | undefined {
  const meters = useContext(MetersContext);
  return bus === "encode" ? meters.encode[id] : meters.playout[id];
}
