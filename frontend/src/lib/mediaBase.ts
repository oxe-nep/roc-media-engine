/** Same-origin media base URL (k3s nginx proxy vs local dev backend). */
export function mediaBase(): string {
  const base = (process.env.NEXT_PUBLIC_BACKEND_URL ?? "").trim();
  if (!base) {
    if (typeof window !== "undefined") return window.location.origin;
    return "http://localhost:8080";
  }
  return base;
}

export function mediaURL(path: string): string {
  return `${mediaBase()}${path}`;
}
