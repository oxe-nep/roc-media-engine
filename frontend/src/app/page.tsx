"use client";

import { useEffect, useState } from "react";
import StreamGrid from "@/components/StreamGrid";
import TcGrid from "@/components/TcGrid";
import DecodeGrid from "@/components/DecodeGrid";
import CommentatorGrid from "@/components/CommentatorGrid";
import SystemStatus from "@/components/SystemStatus";
import LibraryModal from "@/components/LibraryModal";
import SettingsModal from "@/components/SettingsModal";
import { DashboardProvider } from "@/hooks/useDashboard";

export default function Home() {
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [libraryOpen, setLibraryOpen] = useState(false);
  const [anyRecording, setAnyRecording] = useState(false);

  useEffect(() => {
    const openLibrary = () => setLibraryOpen(true);
    const onRecState = (e: Event) => {
      const detail = (e as CustomEvent<{ anyRecording?: boolean }>).detail;
      setAnyRecording(!!detail?.anyRecording);
    };
    window.addEventListener("roc-open-library", openLibrary);
    window.addEventListener("roc-recording-state", onRecState);
    return () => {
      window.removeEventListener("roc-open-library", openLibrary);
      window.removeEventListener("roc-recording-state", onRecState);
    };
  }, []);

  return (
    <DashboardProvider>
      <header className="compact-header">
        <div className="header-brand">
          <img src="/nep-logo.svg" alt="NEP" className="nep-logo" />
        </div>
        <div className="header-actions">
          <SystemStatus />
          <button
            type="button"
            className="header-icon-btn"
            onClick={() => setLibraryOpen(true)}
            title="Media Library"
            aria-label="Media Library"
          >
            <i className="fa-solid fa-folder-open" aria-hidden />
          </button>
          <button
            type="button"
            className="header-icon-btn"
            onClick={() => setSettingsOpen(true)}
            title="Settings"
            aria-label="Settings"
          >
            <i className="fa-solid fa-gear" aria-hidden />
          </button>
        </div>
      </header>
      <StreamGrid />
      <DecodeGrid />
      <TcGrid />
      <CommentatorGrid />
      <SettingsModal
        open={settingsOpen}
        onClose={() => setSettingsOpen(false)}
        anyRecording={anyRecording}
      />
      <LibraryModal
        open={libraryOpen}
        onClose={() => {
          setLibraryOpen(false);
          window.dispatchEvent(new Event("roc-library-changed"));
        }}
      />
    </DashboardProvider>
  );
}
