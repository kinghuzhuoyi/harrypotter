export const IDLE_SUBMIT_MS = 3000;
export const ABSORB_MS = 1200;
export const ANSWER_HOLD_MS = 12000;
export const ANSWER_FADE_MS = 1200;
export const SWIPE_THRESHOLD_PX = 44;

export function characterDelay(character) {
  if (/\n/.test(character)) return 220;
  if (/\s/.test(character)) return 24;
  if (/[，,、；;：:]/.test(character)) return 150;
  if (/[。.!！？?]/.test(character)) return 280;
  return 55;
}

export function isDismissSwipe(start, end, threshold = SWIPE_THRESHOLD_PX) {
  if (!start || !end) return false;
  return Math.hypot(end.x - start.x, end.y - start.y) >= threshold;
}
