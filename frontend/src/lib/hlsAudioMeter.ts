/** Peak-meter HLS audio from a media element; audibility is GainNode-only. */

type TapGraph = {
  source: MediaElementAudioSourceNode;
  gain: GainNode;
  analyserL: AnalyserNode;
  analyserR: AnalyserNode;
  bufL: Float32Array;
  bufR: Float32Array;
};

const graphs = new WeakMap<HTMLMediaElement, TapGraph>();

let sharedCtx: AudioContext | null = null;

function audioCtx(): AudioContext {
  if (!sharedCtx) {
    sharedCtx = new AudioContext();
  }
  return sharedCtx;
}

/** Call from a user gesture so muted HLS metering / unmute works in Chromium. */
export function unlockHlsAudio(): void {
  const ctx = audioCtx();
  void ctx.resume().catch(() => {});
}

export type StereoPeaks = { l: number; r: number };

function peakDb(samples: Float32Array): number {
  let peak = 0;
  for (let i = 0; i < samples.length; i++) {
    const v = Math.abs(samples[i]!);
    if (v > peak) peak = v;
  }
  if (peak < 1e-9) return -90;
  return Math.max(-90, 20 * Math.log10(peak));
}

function ensureGraph(el: HTMLMediaElement): TapGraph {
  const existing = graphs.get(el);
  if (existing) return existing;

  const ctx = audioCtx();
  const source = ctx.createMediaElementSource(el);
  const gain = ctx.createGain();
  gain.gain.value = 0;
  const splitter = ctx.createChannelSplitter(2);
  const analyserL = ctx.createAnalyser();
  const analyserR = ctx.createAnalyser();
  analyserL.fftSize = 2048;
  analyserR.fftSize = 2048;
  analyserL.smoothingTimeConstant = 0;
  analyserR.smoothingTimeConstant = 0;

  // Reroute element audio into the graph. Keep element.muted=false so analysers
  // see samples; silence is gain=0 until the user listens.
  source.connect(gain);
  gain.connect(ctx.destination);
  source.connect(splitter);
  splitter.connect(analyserL, 0);
  splitter.connect(analyserR, 1);

  const graph: TapGraph = {
    source,
    gain,
    analyserL,
    analyserR,
    bufL: new Float32Array(analyserL.fftSize),
    bufR: new Float32Array(analyserR.fftSize),
  };
  graphs.set(el, graph);
  return graph;
}

export type HlsMeterTap = {
  setAudible: (on: boolean) => void;
  sample: () => StereoPeaks;
  disconnect: () => void;
};

/**
 * Tap peaks from a playing listen HLS element. Graph is created once per element.
 * `audible` controls GainNode only — meters keep working while "muted".
 */
export function attachHlsMeter(el: HTMLMediaElement, audible = false): HlsMeterTap {
  void audioCtx().resume().catch(() => {});
  const graph = ensureGraph(el);
  // Element mute silences the MediaElementSource input in Chromium — keep false.
  el.muted = false;
  graph.gain.gain.value = audible ? 1 : 0;

  return {
    setAudible: (on) => {
      el.muted = false;
      graph.gain.gain.value = on ? 1 : 0;
    },
    sample: () => {
      graph.analyserL.getFloatTimeDomainData(graph.bufL);
      graph.analyserR.getFloatTimeDomainData(graph.bufR);
      return { l: peakDb(graph.bufL), r: peakDb(graph.bufR) };
    },
    disconnect: () => {
      graph.gain.gain.value = 0;
    },
  };
}
