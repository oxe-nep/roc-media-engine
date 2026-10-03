import type { StreamDeckWeb } from "@elgato-stream-deck/webhid";
import { getStreamDecks, requestStreamDecks } from "@elgato-stream-deck/webhid";
import type { CommentatorIntercomSlot } from "@/lib/api";
import {
  detectStreamDeckPreset,
  isRoleRelevant,
  roleMapForPage,
  type StreamDeckGridPreset,
  type StreamDeckKeyRole,
} from "@/lib/streamDeckLayout";
import {
  intercomToLayout,
  isIntercomSlotActive,
  STREAM_DECK_VOLUME_STEP,
  type StreamDeckVolumeAdjust,
} from "@/lib/streamDeckBridge";

export function streamDeckWebHIDSupported(): boolean {
  return typeof navigator !== "undefined" && "hid" in navigator;
}

type DeckCallbacks = {
  onPTT: (channel: number) => void;
  onHosta: (active: boolean) => void;
  onVolume: (adjust: StreamDeckVolumeAdjust) => void;
  onPageChange?: (page: 1 | 2) => void;
};

type RenderState = {
  intercom: CommentatorIntercomSlot[];
  pgmVol: number;
  intercomVol: Record<number, number>;
  pttActive: number | null;
  hostaActive: boolean;
  page: 1 | 2;
};

function buttonControl(deck: StreamDeckWeb, col: number, row: number) {
  return deck.CONTROLS.find((c) => c.type === "button" && c.column === col && c.row === row);
}

function gridSize(deck: StreamDeckWeb): { columns: number; rows: number } {
  const buttons = deck.CONTROLS.filter((c) => c.type === "button");
  let columns = 0;
  let rows = 0;
  for (const b of buttons) {
    if (b.column + 1 > columns) columns = b.column + 1;
    if (b.row + 1 > rows) rows = b.row + 1;
  }
  return { columns, rows };
}

function drawKeyCanvas(
  width: number,
  height: number,
  lines: string[],
  bg: string,
  fg = "#ffffff",
): HTMLCanvasElement {
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  if (!ctx) return canvas;
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, width, height);
  ctx.fillStyle = fg;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  const fontSize = Math.max(10, Math.floor(height / (lines.length + 2)));
  ctx.font = `600 ${fontSize}px system-ui, sans-serif`;
  const startY = height / 2 - ((lines.length - 1) * fontSize * 0.55) / 2;
  lines.forEach((line, i) => {
    ctx.fillText(line, width / 2, startY + i * fontSize * 1.1);
  });
  return canvas;
}

export class StreamDeckWebHIDController {
  private deck: StreamDeckWeb | null = null;
  private preset: StreamDeckGridPreset | null = null;
  private page: 1 | 2 = 1;
  private roleByPos = new Map<string, StreamDeckKeyRole>();
  private callbacks: DeckCallbacks | null = null;
  private activePTT: StreamDeckKeyRole | null = null;
  private lastLayout: ReturnType<typeof intercomToLayout> = [];
  private downHandler = (control: { type: string; column: number; row: number }) => {
    if (control.type !== "button") return;
    void this.handleDown(control.column, control.row);
  };
  private upHandler = (control: { type: string; column: number; row: number }) => {
    if (control.type !== "button") return;
    void this.handleUp(control.column, control.row);
  };
  private errorHandler = (err: unknown) => {
    console.error("[streamdeck-webhid]", err);
  };

  get connected(): boolean {
    return this.deck != null;
  }

  get productName(): string | null {
    return this.deck?.PRODUCT_NAME ?? null;
  }

  get presetLabel(): string | null {
    return this.preset?.label ?? null;
  }

  setCallbacks(callbacks: DeckCallbacks | null) {
    this.callbacks = callbacks;
  }

  async connect(requestNew = true): Promise<void> {
    if (!streamDeckWebHIDSupported()) {
      throw new Error("WebHID is not supported in this browser. Use Chrome or Edge.");
    }
    await this.disconnect();
    const decks = requestNew ? await requestStreamDecks() : await getStreamDecks();
    const deck = decks[0];
    if (!deck) {
      throw new Error("No Stream Deck selected.");
    }
    this.deck = deck;
    const size = gridSize(deck);
    this.preset = detectStreamDeckPreset(size.columns, size.rows);
    this.page = 1;
    this.lastLayout = [];
    this.roleByPos.clear();
    deck.on("down", this.downHandler);
    deck.on("up", this.upHandler);
    deck.on("error", this.errorHandler);
  }

  async reconnect(): Promise<void> {
    await this.connect(false);
  }

  async disconnect(): Promise<void> {
    const deck = this.deck;
    if (!deck) return;
    deck.off("down", this.downHandler);
    deck.off("up", this.upHandler);
    deck.off("error", this.errorHandler);
    this.callbacks?.onPTT(0);
    this.callbacks?.onHosta(false);
    this.activePTT = null;
    await deck.close();
    this.deck = null;
    this.preset = null;
    this.roleByPos.clear();
  }

  async render(state: RenderState): Promise<void> {
    const deck = this.deck;
    const preset = this.preset;
    if (!deck || !preset) return;

    const layout = intercomToLayout(state.intercom);
    this.lastLayout = layout;

    if (state.page !== this.page) {
      this.page = state.page;
    }
    // If volume page has nothing to show, stay on intercom page.
    if (this.page === 2 && layout.length === 0) {
      this.page = 1;
      this.callbacks?.onPageChange?.(1);
    }

    this.syncActivePTT(state, layout);
    this.roleByPos = roleMapForPage(preset, this.page, layout);

    const pageKeys = this.page === 1 ? preset.page1 : preset.page2;
    for (const { col, row, role } of pageKeys) {
      const control = buttonControl(deck, col, row);
      if (!control || control.type !== "button") continue;

      if (!isRoleRelevant(role, layout)) {
        await deck.clearKey(control.index);
        continue;
      }

      const pixelSize = control.feedbackType === "lcd" ? control.pixelSize : { width: 72, height: 72 };
      const canvas = this.canvasForRole(role, layout, state, pixelSize.width, pixelSize.height);
      await deck.fillKeyCanvas(control.index, canvas);
    }
  }

  private syncActivePTT(state: RenderState, layout: ReturnType<typeof intercomToLayout>) {
    if (state.pttActive == null) return;
    if (layout.some((b) => b.channel === state.pttActive)) return;
    this.activePTT = null;
    this.callbacks?.onPTT(0);
  }

  private canvasForRole(
    role: StreamDeckKeyRole,
    layout: ReturnType<typeof intercomToLayout>,
    state: RenderState,
    width: number,
    height: number,
  ): HTMLCanvasElement {
    switch (role.kind) {
      case "ptt": {
        const btn = layout.find((b) => b.slot === role.slot);
        if (!btn) {
          return drawKeyCanvas(width, height, [""], "#0a0a0a");
        }
        const label = btn.label.split(/\s+/)[0]?.slice(0, 10) || "PTT";
        const active = state.pttActive === btn.channel;
        return drawKeyCanvas(width, height, ["PTT", label], active ? "#1a7f37" : "#1e293b");
      }
      case "hosta":
        return drawKeyCanvas(
          width,
          height,
          ["HOSTA", "Hold mute"],
          state.hostaActive ? "#b45309" : "#1e293b",
        );
      case "pgm_vol": {
        const arrow = role.direction === "up" ? "+" : "−";
        const pct = Math.round(state.pgmVol * 100);
        return drawKeyCanvas(width, height, [`PGM ${arrow}`, `${pct}%`], "#312e81");
      }
      case "ic_vol": {
        const btn = layout.find((b) => b.slot === role.slot);
        if (!btn) {
          return drawKeyCanvas(width, height, [""], "#0a0a0a");
        }
        const short = btn.label.split(/\s+/)[0]?.slice(0, 8) || "IC";
        const arrow = role.direction === "up" ? "+" : "−";
        const pct = Math.round((state.intercomVol[btn.channel] ?? 0.8) * 100);
        return drawKeyCanvas(width, height, [`${short} ${arrow}`, `${pct}%`], "#134e4a");
      }
      case "page":
        return drawKeyCanvas(
          width,
          height,
          [role.page === 2 ? "Vol →" : "← IC"],
          "#334155",
        );
    }
  }

  private handleDown(col: number, row: number) {
    const role = this.roleByPos.get(`${col},${row}`);
    if (!role || !this.callbacks) return;
    switch (role.kind) {
      case "ptt": {
        const layoutBtn = this.lastLayout.find((b) => b.slot === role.slot);
        if (!layoutBtn) return;
        this.activePTT = role;
        this.callbacks.onPTT(layoutBtn.channel);
        break;
      }
      case "hosta":
        this.callbacks.onHosta(true);
        break;
      case "pgm_vol":
        this.callbacks.onVolume({
          target: "pgm",
          delta: role.direction === "up" ? STREAM_DECK_VOLUME_STEP : -STREAM_DECK_VOLUME_STEP,
        });
        break;
      case "ic_vol": {
        if (!isIntercomSlotActive(this.lastLayout, role.slot)) return;
        this.callbacks.onVolume({
          target: "intercom",
          slot: role.slot,
          delta: role.direction === "up" ? STREAM_DECK_VOLUME_STEP : -STREAM_DECK_VOLUME_STEP,
        });
        break;
      }
      case "page":
        this.page = role.page;
        this.roleByPos = roleMapForPage(this.preset!, this.page, this.lastLayout);
        this.callbacks.onPageChange?.(this.page);
        break;
    }
  }

  private handleUp(col: number, row: number) {
    const role = this.roleByPos.get(`${col},${row}`);
    if (!role || !this.callbacks) return;
    if (role.kind === "ptt" && this.activePTT === role) {
      this.activePTT = null;
      this.callbacks.onPTT(0);
    } else if (role.kind === "hosta") {
      this.callbacks.onHosta(false);
    }
  }
}
