"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import type { CommentatorIntercomSlot } from "@/lib/api";
import type { StreamDeckVolumeAdjust } from "@/lib/streamDeckBridge";
import { StreamDeckWebHIDController, streamDeckWebHIDSupported } from "@/lib/streamDeckWebHID";

type Args = {
  enabled: boolean;
  intercom: CommentatorIntercomSlot[];
  pgmVol: number;
  intercomVol: Record<number, number>;
  pttActive: number | null;
  hostaActive: boolean;
  onVolumeAdjust: (adjust: StreamDeckVolumeAdjust) => void;
  onPTTChange: (channel: number) => void;
  onHostaChange: (active: boolean) => void;
};

export function useStreamDeckWebHID({
  enabled,
  intercom,
  pgmVol,
  intercomVol,
  pttActive,
  hostaActive,
  onVolumeAdjust,
  onPTTChange,
  onHostaChange,
}: Args) {
  const controllerRef = useRef<StreamDeckWebHIDController | null>(null);
  const [connected, setConnected] = useState(false);
  const [deviceName, setDeviceName] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [page, setPage] = useState<1 | 2>(1);

  const onVolumeRef = useRef(onVolumeAdjust);
  const onPTTRef = useRef(onPTTChange);
  const onHostaRef = useRef(onHostaChange);
  onVolumeRef.current = onVolumeAdjust;
  onPTTRef.current = onPTTChange;
  onHostaRef.current = onHostaChange;

  useEffect(() => {
    if (!controllerRef.current) {
      controllerRef.current = new StreamDeckWebHIDController();
    }
    const controller = controllerRef.current;
    controller.setCallbacks({
      onPTT: (channel) => onPTTRef.current(channel),
      onHosta: (active) => onHostaRef.current(active),
      onVolume: (adjust) => onVolumeRef.current(adjust),
      onPageChange: (next) => setPage(next),
    });
    return () => {
      controller.setCallbacks(null);
    };
  }, []);

  useEffect(() => {
    if (!enabled || !connected) return;
    const controller = controllerRef.current;
    if (!controller) return;
    void controller.render({
      intercom,
      pgmVol,
      intercomVol,
      pttActive,
      hostaActive,
      page,
    });
  }, [enabled, connected, intercom, pgmVol, intercomVol, pttActive, hostaActive, page]);

  const connect = useCallback(async (requestNew = true) => {
    setError(null);
    try {
      const controller = controllerRef.current ?? new StreamDeckWebHIDController();
      controllerRef.current = controller;
      await controller.connect(requestNew);
      setConnected(true);
      setDeviceName(controller.productName);
      setPage(1);
    } catch (e) {
      setConnected(false);
      setDeviceName(null);
      setError(e instanceof Error ? e.message : String(e));
      throw e;
    }
  }, []);

  const disconnect = useCallback(async () => {
    await controllerRef.current?.disconnect();
    setConnected(false);
    setDeviceName(null);
    setPage(1);
  }, []);

  useEffect(() => {
    if (!enabled) {
      void disconnect();
    }
  }, [enabled, disconnect]);

  useEffect(() => {
    return () => {
      void controllerRef.current?.disconnect();
    };
  }, []);

  return {
    supported: streamDeckWebHIDSupported(),
    connected,
    deviceName,
    presetLabel: controllerRef.current?.presetLabel ?? null,
    error,
    connect,
    disconnect,
  };
}
