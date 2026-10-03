"use client";

import { useEffect } from "react";

let lockCount = 0;
let savedBodyOverflow = "";

function applyLock() {
  const body = document.body;
  savedBodyOverflow = body.style.overflow;
  // scrollbar-gutter: stable on <html> already reserves the gutter —
  // only hide overflow; do not add padding or the layout shifts.
  body.style.overflow = "hidden";
}

function releaseLock() {
  document.body.style.overflow = savedBodyOverflow;
}

/** Lock page scroll while a modal is open without shifting layout. */
export function useBodyScrollLock(locked: boolean) {
  useEffect(() => {
    if (!locked) return;
    if (lockCount === 0) {
      applyLock();
    }
    lockCount += 1;
    return () => {
      lockCount = Math.max(0, lockCount - 1);
      if (lockCount === 0) {
        releaseLock();
      }
    };
  }, [locked]);
}
