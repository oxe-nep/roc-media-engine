import type { CommentatorIntercomSlot } from "@/lib/api";
import type { CommentatorDevicePrefs } from "@/lib/commentatorPrefs";

export type CommentatorWebcamQuality = {
  width: number;
  height: number;
  frame_rate: number;
  max_bitrate: number;
};

export type CommentatorQuality = {
  to_commentator_video: string;
  to_commentator_audio: string;
  from_commentator_video: string;
  from_commentator_audio: string;
  webcam?: CommentatorWebcamQuality;
};

export type CommentatorJoinInfo = {
  channel_id: number;
  display_name?: string;
  ice_servers: RTCIceServer[];
  intercom: CommentatorIntercomSlot[];
  quality?: CommentatorQuality;
  ws_path: string;
};

export type CommentatorConnectionState =
  | "idle"
  | "joining"
  | "connecting"
  | "negotiating"
  | "connected"
  | "reconnecting"
  | "failed";

/** Live receive/send diagnostics for the program + webcam legs. */
export type CommentatorRTCStats = {
  videoInKbps: number;
  videoInFps: number;
  videoPacketsLost: number;
  videoPacketsReceived: number;
  videoLossPct: number;
  videoJitterMs: number;
  videoFramesDecoded: number;
  videoFramesDropped: number;
  videoFreezeCount: number;
  videoCodec: string;
  audioInKbps: number;
  audioPacketsLost: number;
  iceState: string;
  candidatePair: string;
};

export type CommentatorSessionOptions = {
  devices?: CommentatorDevicePrefs;
  initialPgmVolume?: number;
  initialIntercomVolumes?: Record<number, number>;
};

/** Commentator page always talks to backend via same origin (nginx proxy). */
function commentatorOrigin(): string {
  if (typeof window !== "undefined") return window.location.origin;
  return "";
}

function wsURL(path: string): string {
  const u = new URL(path, commentatorOrigin() || "http://localhost");
  u.protocol = u.protocol === "https:" ? "wss:" : "ws:";
  return u.toString();
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function preferSendCodec(transceiver: RTCRtpTransceiver, mimeType: string) {
  if (typeof RTCRtpSender === "undefined" || !RTCRtpSender.getCapabilities) return;
  const caps = RTCRtpSender.getCapabilities("video");
  if (!caps?.codecs?.length) return;
  const want = mimeType.toLowerCase();
  const preferred = caps.codecs.filter((c) => c.mimeType.toLowerCase() === want);
  if (preferred.length === 0) return;
  const rest = caps.codecs.filter((c) => c.mimeType.toLowerCase() !== want);
  try {
    transceiver.setCodecPreferences([...preferred, ...rest]);
  } catch {
    /* ignore */
  }
}

/** Cap webcam bitrate/framerate from producer quality preset. */
async function constrainWebcamSender(sender: RTCRtpSender, webcam?: CommentatorWebcamQuality) {
  try {
    const params = sender.getParameters();
    if (!params.encodings?.length) {
      params.encodings = [{}];
    }
    const maxBitrate = webcam?.max_bitrate || 1_200_000;
    const maxFramerate = webcam?.frame_rate || 25;
    for (const enc of params.encodings) {
      enc.maxBitrate = maxBitrate;
      enc.maxFramerate = maxFramerate;
    }
    await sender.setParameters(params);
  } catch {
    /* ignore — browser may not support all knobs */
  }
}

function mediaConstraints(
  devices?: CommentatorDevicePrefs,
  webcam?: CommentatorWebcamQuality,
): MediaStreamConstraints {
  const width = webcam?.width || 1280;
  const height = webcam?.height || 720;
  const fps = webcam?.frame_rate || 25;
  const audio: MediaTrackConstraints = {
    echoCancellation: true,
    noiseSuppression: true,
  };
  const video: MediaTrackConstraints = {
    width: { ideal: width },
    height: { ideal: height },
    frameRate: { ideal: fps, max: Math.max(fps, 30) },
  };
  if (devices?.micId) {
    audio.deviceId = { exact: devices.micId };
  }
  if (devices?.camId) {
    video.deviceId = { exact: devices.camId };
  }
  return { audio, video };
}

export type CommentatorJoinErrorCode = "pin_required" | "invalid_pin" | "expired" | "other";

export class CommentatorJoinError extends Error {
  readonly code: CommentatorJoinErrorCode;

  constructor(message: string, code: CommentatorJoinErrorCode = "other") {
    super(message);
    this.name = "CommentatorJoinError";
    this.code = code;
  }
}

export async function fetchCommentatorJoin(token: string, pin: string): Promise<CommentatorJoinInfo> {
  const base = commentatorOrigin();
  const res = await fetch(`${base}/api/commentator/join/${encodeURIComponent(token)}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ pin: pin.trim() }),
  });
  const body = await res.json().catch(() => ({}));
  if (!res.ok) {
    const msg = body.error || `Join failed (${res.status})`;
    if (body.pin_required) throw new CommentatorJoinError(msg, "pin_required");
    if (body.invalid_pin) throw new CommentatorJoinError(msg, "invalid_pin");
    if (body.expired) throw new CommentatorJoinError(msg, "expired");
    throw new CommentatorJoinError(msg, "other");
  }
  return body as CommentatorJoinInfo;
}

type SignalMsg = {
  type: string;
  sdp?: string;
  channel?: number;
  candidate?: RTCIceCandidateInit;
  intercom?: CommentatorIntercomSlot[] | Record<string, number>;
  quality?: CommentatorQuality;
  reconnect_required?: boolean;
  channel_id?: number;
  message?: string;
};

export class CommentatorSession {
  private pc: RTCPeerConnection | null = null;
  private ws: WebSocket | null = null;
  private localStream: MediaStream | null = null;
  private audioEls = new Map<string, HTMLAudioElement>();
  private pendingRemoteICE: RTCIceCandidateInit[] = [];
  private remoteDescSet = false;
  private answering = false;
  private stopped = false;
  private reconnectAttempts = 0;
  private readonly maxReconnects = 3;
  private iceServers: RTCIceServer[] = [];
  private iceTimer: ReturnType<typeof setTimeout> | null = null;
  private statsTimer: ReturnType<typeof setInterval> | null = null;
  private audioUnlocked = false;
  private pgmVolume = 1;
  private intercomVolumes = new Map<number, number>();
  private hostaActive = false;
  private webcamQuality: CommentatorWebcamQuality | undefined;
  private readonly devices: CommentatorDevicePrefs;
  private prevStats: {
    ts: number;
    bytesReceived: number;
    audioBytes: number;
    framesDecoded: number;
  } | null = null;

  onState?: (state: CommentatorConnectionState) => void;
  onError?: (message: string) => void;
  onIntercom?: (slots: CommentatorIntercomSlot[]) => void;
  onReconnectRequired?: (required: boolean) => void;
  onRemoteVideo?: (stream: MediaStream) => void;
  onAudioLocked?: (locked: boolean) => void;
  onStats?: (stats: CommentatorRTCStats) => void;
  onDisplayName?: (name: string) => void;
  onJoin?: (info: CommentatorJoinInfo) => void;

  constructor(
    private readonly token: string,
    private readonly pin: string,
    options: CommentatorSessionOptions = {},
  ) {
    this.devices = options.devices ?? { micId: "", camId: "" };
    this.pgmVolume = options.initialPgmVolume ?? 1;
    if (options.initialIntercomVolumes) {
      for (const [id, vol] of Object.entries(options.initialIntercomVolumes)) {
        this.intercomVolumes.set(Number(id), vol);
      }
    }
  }

  async start(): Promise<void> {
    this.stopped = false;
    this.remoteDescSet = false;
    this.answering = false;
    this.pendingRemoteICE = [];
    this.onState?.("joining");

    const join = await fetchCommentatorJoin(this.token, this.pin);
    this.onJoin?.(join);
    this.iceServers = join.ice_servers;
    this.onIntercom?.(join.intercom);
    if (join.display_name?.trim()) {
      this.onDisplayName?.(join.display_name.trim());
    }
    if (join.quality?.webcam) {
      this.webcamQuality = join.quality.webcam;
    }

    this.onState?.("connecting");
    await this.connectSignaling(join.ws_path);
  }

  private async scheduleReconnect() {
    if (this.stopped || this.reconnectAttempts >= this.maxReconnects) {
      this.onState?.("failed");
      this.onError?.(
        "WebRTC connection failed. Check WEBRTC_PUBLIC_HOST (and TURN if remote) on the capture host, then click Reconnect.",
      );
      return;
    }
    this.reconnectAttempts++;
    this.onState?.("reconnecting");
    this.ws?.close();
    this.pc?.close();
    this.ws = null;
    this.pc = null;
    await delay(Math.min(1500 * this.reconnectAttempts, 5000));
    if (this.stopped) return;
    try {
      await this.start();
    } catch {
      void this.scheduleReconnect();
    }
  }

  private signalingURL(path: string): string {
    const url = wsURL(path);
    const pin = this.pin.trim();
    if (!pin) return url;
    const u = new URL(url);
    u.searchParams.set("pin", pin);
    return u.toString();
  }

  private async connectSignaling(path: string): Promise<void> {
    const url = this.signalingURL(path);
    await new Promise<void>((resolve, reject) => {
      let settled = false;
      const ws = new WebSocket(url);
      this.ws = ws;
      ws.onopen = () => {
        settled = true;
        resolve();
      };
      ws.onclose = (ev) => {
        if (!settled) {
          const hint =
            path.includes("/api/commentator/ws/")
              ? " Redeploy frontend and capture host, then create a new invite."
              : "";
          reject(
            new Error(
              `Signaling WebSocket closed (${ev.code}${ev.reason ? `: ${ev.reason}` : ""}). URL: ${url}.${hint}`,
            ),
          );
        }
      };
      ws.onerror = () => {
        if (!settled) {
          reject(
            new Error(
              `Signaling WebSocket failed (${url}). If the dashboard works but this does not, redeploy frontend + capture host and create a new invite.`,
            ),
          );
        }
      };
      ws.onmessage = (ev) => void this.handleSignal(JSON.parse(String(ev.data)) as SignalMsg);
    });
  }

  private markConnected() {
    if (this.iceTimer) {
      clearTimeout(this.iceTimer);
      this.iceTimer = null;
    }
    this.reconnectAttempts = 0;
    this.onState?.("connected");
    void this.unlockAudio();
    this.applyAllVolumes();
    this.startStatsPolling();
  }

  private startStatsPolling() {
    if (this.statsTimer) clearInterval(this.statsTimer);
    this.prevStats = null;
    void this.pollStats();
    this.statsTimer = setInterval(() => void this.pollStats(), 2000);
  }

  private async pollStats() {
    if (!this.pc || !this.onStats) return;
    try {
      const report = await this.pc.getStats();
      let inboundVideo: RTCInboundRtpStreamStats | undefined;
      let inboundAudio: RTCInboundRtpStreamStats | undefined;
      const codecMap = new Map<string, string>();
      let pairLabel = "";
      const iceState = this.pc.iceConnectionState;

      report.forEach((r) => {
        if (r.type === "codec") {
          const c = r as unknown as { id: string; mimeType?: string };
          if (c.mimeType) codecMap.set(c.id, c.mimeType);
        }
        if (r.type === "inbound-rtp") {
          const s = r as RTCInboundRtpStreamStats;
          if (s.kind === "video" || (s as { mediaType?: string }).mediaType === "video") {
            inboundVideo = s;
          } else if (s.kind === "audio" || (s as { mediaType?: string }).mediaType === "audio") {
            if (!inboundAudio) inboundAudio = s;
          }
        }
        if (r.type === "candidate-pair") {
          const p = r as RTCIceCandidatePairStats;
          if (p.state === "succeeded" && p.nominated) {
            const local = report.get(p.localCandidateId || "") as
              | { candidateType?: string }
              | undefined;
            const remote = report.get(p.remoteCandidateId || "") as
              | { candidateType?: string; address?: string; ip?: string; port?: number }
              | undefined;
            const addr = remote?.address || remote?.ip || "";
            pairLabel = `${local?.candidateType || "?"}->${remote?.candidateType || "?"} ${addr}:${remote?.port || ""}`;
          }
        }
      });

      const now = performance.now();
      const bytes = inboundVideo?.bytesReceived ?? 0;
      const audioBytes = inboundAudio?.bytesReceived ?? 0;
      const framesDecoded = inboundVideo?.framesDecoded ?? 0;
      let videoInKbps = 0;
      let videoInFps = 0;
      let audioInKbps = 0;
      if (this.prevStats) {
        const dt = (now - this.prevStats.ts) / 1000;
        if (dt > 0.2) {
          videoInKbps = ((bytes - this.prevStats.bytesReceived) * 8) / dt / 1000;
          audioInKbps = ((audioBytes - this.prevStats.audioBytes) * 8) / dt / 1000;
          videoInFps = (framesDecoded - this.prevStats.framesDecoded) / dt;
        }
      }
      this.prevStats = { ts: now, bytesReceived: bytes, audioBytes, framesDecoded };

      const lost = inboundVideo?.packetsLost ?? 0;
      const received = inboundVideo?.packetsReceived ?? 0;
      const total = lost + received;
      const videoLossPct = total > 0 ? (lost / total) * 100 : 0;
      const codecId = inboundVideo?.codecId || "";
      const videoCodec = codecMap.get(codecId) || "—";

      this.onStats({
        videoInKbps: Math.max(0, Math.round(videoInKbps)),
        videoInFps: Math.max(0, Math.round(videoInFps * 10) / 10),
        videoPacketsLost: lost,
        videoPacketsReceived: received,
        videoLossPct: Math.round(videoLossPct * 10) / 10,
        videoJitterMs: Math.round((inboundVideo?.jitter ?? 0) * 1000),
        videoFramesDecoded: framesDecoded,
        videoFramesDropped: inboundVideo?.framesDropped ?? 0,
        videoFreezeCount: (inboundVideo as { freezeCount?: number })?.freezeCount ?? 0,
        videoCodec,
        audioInKbps: Math.max(0, Math.round(audioInKbps)),
        audioPacketsLost: inboundAudio?.packetsLost ?? 0,
        iceState,
        candidatePair: pairLabel || "—",
      });
    } catch {
      /* ignore */
    }
  }

  private startIceTimeout() {
    if (this.iceTimer) clearTimeout(this.iceTimer);
    this.iceTimer = setTimeout(() => {
      if (this.stopped || this.pc?.connectionState === "connected") return;
      const ice = this.pc?.iceConnectionState;
      if (ice === "connected" || ice === "completed") return;
      this.onError?.(
        "Media connection timed out (ICE). On the capture host set WEBRTC_PUBLIC_HOST to a reachable IP/hostname and configure TURN for remote commentators.",
      );
      this.onState?.("failed");
    }, 25000);
  }

  /** Browsers block unmuted autoplay until a user gesture — call from UI clicks too. */
  async unlockAudio(): Promise<void> {
    const plays: Promise<void>[] = [];
    for (const el of this.audioEls.values()) {
      plays.push(
        el.play().then(() => {
          this.audioUnlocked = true;
        }),
      );
    }
    await Promise.allSettled(plays);
    this.onAudioLocked?.(!this.audioUnlocked && this.audioEls.size > 0);
  }

  private applyVolumeToElement(id: string, el: HTMLAudioElement) {
    if (id.includes("pgm") || id === "audio") {
      el.volume = this.pgmVolume;
      return;
    }
    const m = id.match(/intercom(\d+)/);
    if (m) {
      const slotId = Number(m[1]);
      el.volume = this.intercomVolumes.get(slotId) ?? 0.8;
    }
  }

  private applyAllVolumes() {
    for (const [id, el] of this.audioEls) {
      this.applyVolumeToElement(id, el);
    }
  }

  private bindRemoteAudio(track: MediaStreamTrack, stream: MediaStream) {
    const id = stream.id || track.id || track.label || `audio-${this.audioEls.size}`;
    const existing = this.audioEls.get(id);
    if (existing) {
      existing.srcObject = new MediaStream([track]);
      this.applyVolumeToElement(id, existing);
      void existing.play().catch(() => this.onAudioLocked?.(true));
      return;
    }
    const el = document.createElement("audio");
    el.autoplay = true;
    el.setAttribute("playsinline", "true");
    el.srcObject = new MediaStream([track]);
    this.applyVolumeToElement(id, el);
    el.style.display = "none";
    document.body.appendChild(el);
    this.audioEls.set(id, el);
    void el.play().then(
      () => {
        this.audioUnlocked = true;
        this.onAudioLocked?.(false);
      },
      () => this.onAudioLocked?.(true),
    );
  }

  private bindPeerConnectionHandlers() {
    if (!this.pc) return;
    this.pc.oniceconnectionstatechange = () => {
      const ice = this.pc?.iceConnectionState;
      if (ice === "connected" || ice === "completed") {
        this.markConnected();
      } else if (ice === "failed") {
        void this.scheduleReconnect();
      }
    };
    this.pc.onconnectionstatechange = () => {
      const state = this.pc?.connectionState;
      if (state === "connected") {
        this.markConnected();
      } else if (state === "failed") {
        void this.scheduleReconnect();
      }
    };
  }

  private async acquireLocalStream() {
    if (this.localStream) {
      this.localStream.getTracks().forEach((t) => t.stop());
      this.localStream = null;
    }
    this.localStream = await navigator.mediaDevices.getUserMedia(
      mediaConstraints(this.devices, this.webcamQuality),
    );
    this.applyHosta();
  }

  private applyHosta() {
    const track = this.localStream?.getAudioTracks()[0];
    if (track) {
      track.enabled = !this.hostaActive;
    }
  }

  private async handleOffer(sdp: string) {
    if (this.answering || this.stopped) return;
    this.answering = true;
    this.onState?.("negotiating");
    try {
      this.pc = new RTCPeerConnection({ iceServers: this.iceServers });
      this.bindPeerConnectionHandlers();

      await this.acquireLocalStream();

      this.pc.onicecandidate = (ev) => {
        if (ev.candidate) {
          this.send({ type: "ice", candidate: ev.candidate.toJSON() });
        }
      };
      this.pc.ontrack = (ev) => {
        if (ev.track.kind === "video") {
          this.onRemoteVideo?.(ev.streams[0] ?? new MediaStream([ev.track]));
          return;
        }
        const stream = ev.streams[0] ?? new MediaStream([ev.track]);
        this.bindRemoteAudio(ev.track, stream);
      };

      await this.pc.setRemoteDescription({ type: "offer", sdp });
      this.remoteDescSet = true;

      const videoTrack = this.localStream!.getVideoTracks()[0];
      const audioTrack = this.localStream!.getAudioTracks()[0];
      if (videoTrack) this.pc.addTrack(videoTrack, this.localStream!);
      if (audioTrack) this.pc.addTrack(audioTrack, this.localStream!);

      for (const tx of this.pc.getTransceivers()) {
        if (tx.sender.track?.kind !== "video") continue;
        preferSendCodec(tx, "video/H264");
        if (tx.sender) await constrainWebcamSender(tx.sender, this.webcamQuality);
      }

      const answer = await this.pc.createAnswer();
      await this.pc.setLocalDescription(answer);
      this.send({ type: "answer", sdp: answer.sdp });
      this.onState?.("connecting");
      this.startIceTimeout();

      for (const c of this.pendingRemoteICE) {
        await this.pc.addIceCandidate(c);
      }
      this.pendingRemoteICE = [];
    } catch (e) {
      const detail = e instanceof Error ? e.message : String(e);
      this.onError?.(`WebRTC negotiation failed: ${detail}`);
      this.onState?.("failed");
    }
  }

  private async handleSignal(msg: SignalMsg) {
    switch (msg.type) {
      case "config":
        if (Array.isArray(msg.intercom)) this.onIntercom?.(msg.intercom);
        if (msg.quality?.webcam) this.webcamQuality = msg.quality.webcam;
        this.onReconnectRequired?.(!!msg.reconnect_required);
        return;
      case "offer":
        if (msg.sdp) await this.handleOffer(msg.sdp);
        return;
      case "error":
        this.onError?.(msg.message || "WebRTC error");
        this.onState?.("failed");
        return;
      case "ice":
        if (!msg.candidate) return;
        if (!this.pc || !this.remoteDescSet) {
          this.pendingRemoteICE.push(msg.candidate);
          return;
        }
        await this.pc.addIceCandidate(msg.candidate);
        break;
    }
  }

  setPGMVolume(value: number) {
    this.pgmVolume = value;
    void this.unlockAudio();
    this.applyAllVolumes();
  }

  setIntercomVolume(slotId: number, value: number) {
    this.intercomVolumes.set(slotId, value);
    void this.unlockAudio();
    for (const [id, el] of this.audioEls) {
      if (id.includes(`intercom${slotId}`)) {
        el.volume = value;
      }
    }
  }

  setHosta(active: boolean) {
    this.hostaActive = active;
    this.applyHosta();
  }

  setPTT(channel: number) {
    void this.unlockAudio();
    this.send({ type: "ptt", channel });
  }

  isSignalingOpen(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }

  private send(msg: SignalMsg) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
    }
  }

  stop() {
    this.stopped = true;
    if (this.iceTimer) {
      clearTimeout(this.iceTimer);
      this.iceTimer = null;
    }
    if (this.statsTimer) {
      clearInterval(this.statsTimer);
      this.statsTimer = null;
    }
    this.send({ type: "ptt", channel: 0 });
    this.ws?.close();
    this.pc?.close();
    this.localStream?.getTracks().forEach((t) => t.stop());
    for (const el of this.audioEls.values()) {
      el.pause();
      el.srcObject = null;
      el.remove();
    }
    this.audioEls.clear();
    this.ws = null;
    this.pc = null;
    this.localStream = null;
  }
}

/** Turn a stored invite path into an absolute browser URL. */
export function absoluteInviteURL(url: string): string {
  const u = url.trim();
  if (!u) return "";
  if (u.startsWith("http://") || u.startsWith("https://")) return u;
  if (typeof window !== "undefined") {
    return `${window.location.origin}${u.startsWith("/") ? u : `/${u}`}`;
  }
  return u;
}

export async function listMediaDevices(): Promise<{
  mics: MediaDeviceInfo[];
  cams: MediaDeviceInfo[];
}> {
  if (!navigator.mediaDevices?.enumerateDevices) {
    return { mics: [], cams: [] };
  }
  const devices = await navigator.mediaDevices.enumerateDevices();
  return {
    mics: devices.filter((d) => d.kind === "audioinput"),
    cams: devices.filter((d) => d.kind === "videoinput"),
  };
}
