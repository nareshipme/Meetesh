const pad = (n: number) => String(n).padStart(2, "0");

/** 52_000 -> "0:52", 3_725_000 -> "1:02:05" */
export function fmtDuration(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  return h > 0 ? `${h}:${pad(m)}:${pad(s % 60)}` : `${m}:${pad(s % 60)}`;
}

/** Friendly date for lists: "Today 21:59", "Yesterday 09:30", "21 Nov, 21:59", "21 Nov 2025, 21:59". */
export function fmtWhen(unixSeconds: number, now: Date = new Date()): string {
  const d = new Date(unixSeconds * 1000);
  const time = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  const dayStart = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const days = Math.round((dayStart(now) - dayStart(d)) / 86_400_000);
  if (days === 0) return `Today ${time}`;
  if (days === 1) return `Yesterday ${time}`;
  const date = d.toLocaleDateString(undefined, {
    day: "numeric",
    month: "short",
    ...(d.getFullYear() !== now.getFullYear() ? { year: "numeric" } : {}),
  });
  return `${date}, ${time}`;
}

/**
 * RMS (0..1) to a 0..1 meter position on a dB scale, so normal speech (about -35 dBFS) lands
 * mid-way instead of near zero. -60 dBFS is empty, -10 dBFS is full.
 */
export function levelToFraction(rms: number): number {
  const db = 20 * Math.log10(Math.max(rms, 1e-5));
  return Math.min(1, Math.max(0, (db + 60) / 50));
}

/** Rust errors arrive as plain strings, sometimes wrapped as "Error: ...". */
export function errorText(e: unknown): string {
  const s = e instanceof Error ? e.message : String(e);
  return s.replace(/^Error:\s*/, "");
}
