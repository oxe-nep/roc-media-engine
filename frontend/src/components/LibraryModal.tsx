"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import {
  createLibraryCategory,
  deleteLibraryCategory,
  deleteLibraryFile,
  deletePlayoutMedia,
  fetchLibraryCategories,
  fetchLibraryFiles,
  fetchPlayoutMedia,
  libraryFileURL,
  moveLibraryFile,
  renameLibraryCategory,
  uploadPlayoutMedia,
  type LibraryCategory,
  type LibraryFile,
  type PlayoutMediaItem,
} from "@/lib/api";
import { useBodyScrollLock } from "@/hooks/useBodyScrollLock";

const UPLOADS_SECTION = "__uploads__";

type Props = {
  open: boolean;
  onClose: () => void;
  /** When set, picking a file calls onPick and closes (for decode File mode). */
  pickMode?: boolean;
  onPick?: (file: LibraryFile) => void;
};

function formatUploadSize(n: number): string {
  if (n >= 1 << 30) return `${(n / (1 << 30)).toFixed(1)} GB`;
  if (n >= 1 << 20) return `${(n / (1 << 20)).toFixed(1)} MB`;
  if (n >= 1 << 10) return `${(n / (1 << 10)).toFixed(0)} KB`;
  return `${n} B`;
}

export default function LibraryModal({ open, onClose, pickMode, onPick }: Props) {
  const [categories, setCategories] = useState<LibraryCategory[]>([]);
  const [files, setFiles] = useState<LibraryFile[]>([]);
  const [uploads, setUploads] = useState<PlayoutMediaItem[]>([]);
  const [selectedCat, setSelectedCat] = useState<string>("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [uploadBusy, setUploadBusy] = useState(false);
  const [newCat, setNewCat] = useState("");
  const [playerURL, setPlayerURL] = useState<string | null>(null);
  const [playingKey, setPlayingKey] = useState<string | null>(null);
  const uploadInputRef = useRef<HTMLInputElement>(null);

  useBodyScrollLock(open);

  const showingUploads = !pickMode && selectedCat === UPLOADS_SECTION;

  const load = useCallback(async () => {
    try {
      const [cats, list, media] = await Promise.all([
        fetchLibraryCategories(),
        fetchLibraryFiles(selectedCat && selectedCat !== UPLOADS_SECTION ? selectedCat : undefined),
        fetchPlayoutMedia(),
      ]);
      setCategories(cats);
      setFiles(list);
      setUploads(media);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [selectedCat]);

  const notifyLibraryChanged = () => {
    window.dispatchEvent(new Event("roc-library-changed"));
  };

  useEffect(() => {
    if (!open) return;
    setLoading(true);
    load();
    const t = setInterval(load, 5000);
    return () => {
      clearInterval(t);
    };
  }, [open, load]);

  useEffect(() => {
    if (open) return;
    setPlayerURL(null);
    setPlayingKey(null);
    setSelectedCat("");
    notifyLibraryChanged();
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !uploadBusy) onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose, uploadBusy]);

  if (!open) return null;

  const fileKey = (f: LibraryFile) => `${f.category}/${f.name}`;

  const playFile = (f: LibraryFile) => {
    setPlayerURL(libraryFileURL(f.category, f.name));
    setPlayingKey(fileKey(f));
  };

  const downloadFile = (f: LibraryFile) => {
    const a = document.createElement("a");
    a.href = libraryFileURL(f.category, f.name, { download: true });
    a.download = f.name;
    a.rel = "noopener";
    document.body.appendChild(a);
    a.click();
    a.remove();
  };

  const removeFile = async (f: LibraryFile) => {
    if (!window.confirm(`Delete "${f.name}" from ${f.category}?`)) return;
    const key = fileKey(f);
    setBusyKey(key);
    try {
      await deleteLibraryFile(f.category, f.name);
      if (playingKey === key) {
        setPlayerURL(null);
        setPlayingKey(null);
      }
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyKey(null);
    }
  };

  const moveFile = async (f: LibraryFile, toCategory: string) => {
    if (!toCategory || toCategory === f.category) return;
    const key = fileKey(f);
    setBusyKey(key);
    try {
      await moveLibraryFile(f.category, toCategory, f.name);
      await load();
      notifyLibraryChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyKey(null);
    }
  };

  const addCategory = async () => {
    const name = newCat.trim();
    if (!name) return;
    try {
      await createLibraryCategory(name);
      setNewCat("");
      await load();
      notifyLibraryChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  const renameCategory = async (name: string) => {
    const next = window.prompt("Rename category", name);
    if (!next || next.trim() === name) return;
    try {
      await renameLibraryCategory(name, next.trim());
      if (selectedCat === name) setSelectedCat(next.trim().replace(/\s+/g, "_"));
      await load();
      notifyLibraryChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  const removeCategory = async (name: string) => {
    if (!window.confirm(`Delete empty category "${name}"?`)) return;
    try {
      await deleteLibraryCategory(name);
      if (selectedCat === name) setSelectedCat("");
      await load();
      notifyLibraryChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  const onUpload = async (file: File | null) => {
    if (!file) return;
    setUploadBusy(true);
    setError(null);
    try {
      await uploadPlayoutMedia(file);
      setSelectedCat(UPLOADS_SECTION);
      await load();
      notifyLibraryChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setUploadBusy(false);
      if (uploadInputRef.current) uploadInputRef.current.value = "";
    }
  };

  const removeUpload = async (id: string, name: string) => {
    if (!window.confirm(`Delete “${name}” from uploads?`)) return;
    setBusyKey(id);
    try {
      await deletePlayoutMedia(id);
      await load();
      notifyLibraryChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyKey(null);
    }
  };

  const modalTitle = pickMode ? "Pick file" : "Media Library";

  return (
    <div className="modal-backdrop library-backdrop" onClick={() => !uploadBusy && onClose()} role="presentation">
      <div
        className="modal-panel library-modal"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-label={modalTitle}
      >
        <div className="modal-header">
          <h2>{modalTitle}</h2>
          <div className="library-modal-header-actions">
            {!pickMode && (
              <>
                <input
                  ref={uploadInputRef}
                  type="file"
                  accept=".mp4,.mov,.mkv,.mxf,.ts,video/*"
                  hidden
                  onChange={(e) => onUpload(e.target.files?.[0] ?? null)}
                />
                <button
                  type="button"
                  className="global-rec-btn"
                  disabled={loading || uploadBusy}
                  onClick={() => uploadInputRef.current?.click()}
                >
                  {uploadBusy ? "…" : "Upload"}
                </button>
              </>
            )}
            <button type="button" className="badge files-btn" onClick={load} disabled={loading || uploadBusy} title="Refresh">
              {loading ? "…" : "↻"}
            </button>
            <button type="button" className="modal-close" onClick={onClose} aria-label="Close" disabled={uploadBusy}>
              ×
            </button>
          </div>
        </div>

        {error && <div className="error-message">{error}</div>}

        <div className={`library-layout${showingUploads ? " library-layout-uploads" : ""}`}>
          <aside className="library-sidebar">
            <div className="library-sidebar-title">Categories</div>
            <button
              type="button"
              className={`library-cat ${selectedCat === "" ? "active" : ""}`}
              onClick={() => setSelectedCat("")}
            >
              All
            </button>
            {categories.map((c) => (
              <div key={c.name} className="library-cat-row">
                <button
                  type="button"
                  className={`library-cat ${selectedCat === c.name ? "active" : ""}`}
                  onClick={() => setSelectedCat(c.name)}
                >
                  <span>{c.name === "_unsorted" ? "Unsorted" : c.name}</span>
                  <span className="library-cat-count">{c.file_count}</span>
                </button>
                {c.name !== "_unsorted" && (
                  <div className="library-cat-ops">
                    <button type="button" title="Rename" onClick={() => renameCategory(c.name)}>
                      ✎
                    </button>
                    <button type="button" title="Delete if empty" onClick={() => removeCategory(c.name)}>
                      ×
                    </button>
                  </div>
                )}
              </div>
            ))}
            {!pickMode && (
              <button
                type="button"
                className={`library-cat ${showingUploads ? "active" : ""}`}
                onClick={() => setSelectedCat(UPLOADS_SECTION)}
              >
                <span>Uploads</span>
                <span className="library-cat-count">{uploads.length}</span>
              </button>
            )}
            {!pickMode && (
              <div className="library-new-cat">
                <input
                  value={newCat}
                  onChange={(e) => setNewCat(e.target.value)}
                  placeholder="New category"
                  onKeyDown={(e) => {
                    if (e.key === "Enter") addCategory();
                  }}
                />
                <button type="button" className="badge files-btn" onClick={addCategory}>
                  Add
                </button>
              </div>
            )}
          </aside>

          <div className="recordings-list">
            {showingUploads ? (
              uploads.length === 0 ? (
                <div className="files-empty">—</div>
              ) : (
                uploads.map((it) => (
                  <div key={it.id} className="recording-row">
                    <div className="recording-meta">
                      <div className="recording-name">{it.name}</div>
                      <div className="recording-sub">{formatUploadSize(it.size)}</div>
                    </div>
                    <div className="recording-actions">
                      <button
                        type="button"
                        className="badge delete-btn"
                        disabled={busyKey === it.id || uploadBusy}
                        onClick={() => removeUpload(it.id, it.name)}
                      >
                        {busyKey === it.id ? "…" : "Delete"}
                      </button>
                    </div>
                  </div>
                ))
              )
            ) : files.length === 0 ? (
              <div className="files-empty">—</div>
            ) : (
              files.map((f) => {
                const key = fileKey(f);
                return (
                  <div key={key} className={`recording-row ${playingKey === key ? "active" : ""}`}>
                    <div className="recording-meta">
                      <div className="recording-name">{f.name}</div>
                      <div className="recording-sub">
                        {f.category === "_unsorted" ? "Unsorted" : f.category}
                        {" · "}
                        {Math.round(f.size / 1024 / 1024)} MB
                      </div>
                    </div>
                    <div className="recording-actions">
                      {pickMode ? (
                        <button
                          type="button"
                          className="global-rec-btn"
                          onClick={() => {
                            onPick?.(f);
                            onClose();
                          }}
                        >
                          Pick
                        </button>
                      ) : (
                        <>
                          <select
                            className="encode-preset-select"
                            value=""
                            disabled={busyKey === key}
                            onChange={(e) => {
                              const v = e.target.value;
                              e.target.value = "";
                              if (v) moveFile(f, v);
                            }}
                            title="Move to category"
                          >
                            <option value="">Move…</option>
                            {categories
                              .filter((c) => c.name !== f.category)
                              .map((c) => (
                                <option key={c.name} value={c.name}>
                                  {c.name === "_unsorted" ? "Unsorted" : c.name}
                                </option>
                              ))}
                          </select>
                          <button
                            className="badge files-btn"
                            onClick={() => playFile(f)}
                            disabled={busyKey === key}
                          >
                            {busyKey === key ? "…" : "Play"}
                          </button>
                          <button className="badge" onClick={() => downloadFile(f)} disabled={busyKey === key}>
                            Download
                          </button>
                          <button className="badge delete-btn" onClick={() => removeFile(f)} disabled={busyKey === key}>
                            Delete
                          </button>
                        </>
                      )}
                    </div>
                  </div>
                );
              })
            )}
          </div>

          {!pickMode && !showingUploads && (
            <div className="recordings-player-wrap">
              {playerURL ? (
                <video className="recordings-player" controls autoPlay src={playerURL} />
              ) : (
                <div className="files-empty files-empty-muted" aria-hidden />
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
