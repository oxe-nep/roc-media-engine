export type CommentatorSavedVolumes = {
  pgm: number;
  intercom: Record<string, number>;
};

export type CommentatorDevicePrefs = {
  micId: string;
  camId: string;
};

const volumesKey = (token: string) => `roc-commentator-volumes:${token}`;
const debugKey = (token: string) => `roc-commentator-debug:${token}`;
const devicesKey = () => "roc-commentator-devices";

export function loadCommentatorVolumes(token: string): CommentatorSavedVolumes {
  try {
    const raw = localStorage.getItem(volumesKey(token));
    if (!raw) return { pgm: 1, intercom: {} };
    const parsed = JSON.parse(raw) as Partial<CommentatorSavedVolumes>;
    const pgm = typeof parsed.pgm === "number" ? clamp01(parsed.pgm) : 1;
    const intercom: Record<string, number> = {};
    if (parsed.intercom && typeof parsed.intercom === "object") {
      for (const [id, v] of Object.entries(parsed.intercom)) {
        if (typeof v === "number") intercom[id] = clamp01(v);
      }
    }
    return { pgm, intercom };
  } catch {
    return { pgm: 1, intercom: {} };
  }
}

export function saveCommentatorVolumes(
  token: string,
  pgm: number,
  intercom: Record<number, number>,
): void {
  try {
    const payload: CommentatorSavedVolumes = {
      pgm: clamp01(pgm),
      intercom: Object.fromEntries(
        Object.entries(intercom).map(([k, v]) => [k, clamp01(v)]),
      ),
    };
    localStorage.setItem(volumesKey(token), JSON.stringify(payload));
  } catch {
    /* private mode / quota */
  }
}

export function loadCommentatorDebug(token: string): boolean {
  try {
    return localStorage.getItem(debugKey(token)) === "1";
  } catch {
    return false;
  }
}

export function saveCommentatorDebug(token: string, on: boolean): void {
  try {
    localStorage.setItem(debugKey(token), on ? "1" : "0");
  } catch {
    /* ignore */
  }
}

export function loadCommentatorDevices(): CommentatorDevicePrefs {
  try {
    const raw = localStorage.getItem(devicesKey());
    if (!raw) return { micId: "", camId: "" };
    const parsed = JSON.parse(raw) as Partial<CommentatorDevicePrefs>;
    return {
      micId: typeof parsed.micId === "string" ? parsed.micId : "",
      camId: typeof parsed.camId === "string" ? parsed.camId : "",
    };
  } catch {
    return { micId: "", camId: "" };
  }
}

export function saveCommentatorDevices(prefs: CommentatorDevicePrefs): void {
  try {
    localStorage.setItem(devicesKey(), JSON.stringify(prefs));
  } catch {
    /* ignore */
  }
}

const pinKey = (token: string) => `roc-commentator-pin:${token}`;

export function loadCommentatorPin(token: string): string {
  try {
    return sessionStorage.getItem(pinKey(token)) ?? "";
  } catch {
    return "";
  }
}

export function saveCommentatorPin(token: string, pin: string): void {
  try {
    sessionStorage.setItem(pinKey(token), pin.trim());
  } catch {
    /* ignore */
  }
}

export function clearCommentatorPin(token: string): void {
  try {
    sessionStorage.removeItem(pinKey(token));
  } catch {
    /* ignore */
  }
}

function clamp01(v: number): number {
  return Math.min(1, Math.max(0, v));
}
