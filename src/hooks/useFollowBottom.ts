import { useCallback, useEffect, useRef, useState } from "react";

const NEAR_BOTTOM_PX = 48;

/**
 * Keeps a scrolling list pinned to its newest line while `active` (a recording is running), unless the
 * reader scrolled up. It re-pins when new lines arrive and also when the list or its box changes
 * size (a hint or the notes panel appearing above it shrinks the box without adding a line).
 * `jump` returns to the bottom and resumes following.
 */
export function useFollowBottom<T extends HTMLElement>(active: boolean, deps: unknown[]) {
  const ref = useRef<T>(null);
  const following = useRef(true);
  const [behind, setBehind] = useState(false); // scrolled away from the newest line while it is growing

  const pin = useCallback(() => {
    const el = ref.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, []);

  // New lines.
  useEffect(() => {
    if (!active) return;
    if (following.current) pin();
    else setBehind(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active, ...deps]);

  // The box or its content changing size.
  useEffect(() => {
    const el = ref.current;
    if (!el || !active || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => following.current && pin());
    ro.observe(el);
    if (el.firstElementChild) ro.observe(el.firstElementChild);
    return () => ro.disconnect();
  }, [active, pin]);

  const onScroll = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    const near = el.scrollHeight - el.scrollTop - el.clientHeight < NEAR_BOTTOM_PX;
    following.current = near;
    if (near) setBehind(false);
  }, []);

  const jump = useCallback(() => {
    following.current = true;
    setBehind(false);
    pin();
  }, [pin]);

  // A different meeting starts at the top, not following.
  const reset = useCallback(() => {
    following.current = true;
    setBehind(false);
  }, []);

  return { ref, onScroll, behind: active && behind, jump, reset };
}
