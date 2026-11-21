import { useEffect, useRef, useState } from "react";
import { levelToFraction } from "../lib/format";

const SILENCE_HINT_AFTER_MS = 4000;

/** Microphone level on a dB scale with quick attack and slow release, plus a hint if nothing is heard. */
export default function LevelMeter({ level }: { level: number }) {
  const [shown, setShown] = useState(0);
  const [silent, setSilent] = useState(false);
  const lastSound = useRef(Date.now());

  useEffect(() => {
    const f = levelToFraction(level);
    setShown((prev) => Math.max(f, prev * 0.85));
    if (f > 0.08) lastSound.current = Date.now();
  }, [level]);

  useEffect(() => {
    const t = setInterval(() => setSilent(Date.now() - lastSound.current > SILENCE_HINT_AFTER_MS), 1000);
    return () => clearInterval(t);
  }, []);

  return (
    <div className="meter-wrap">
      <div
        className="meter"
        role="meter"
        aria-label="Microphone level"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(shown * 100)}
      >
        <div className="meter-fill" style={{ width: `${shown * 100}%` }} />
      </div>
      {silent && (
        <small className="warn">No sound from the microphone. Check the selected microphone and that it is not muted.</small>
      )}
    </div>
  );
}
