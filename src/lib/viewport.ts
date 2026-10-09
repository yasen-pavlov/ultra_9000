// Reports the single DOM rectangle reserved for the native terminal view and sequences focus after
// it. Every report and focus request is keyed by the native item ID and presentation epoch it was
// made for, plus a monotonic sequence; native rejects stale work and remains the final guard.
import { errorMessage, focusTerminal, setTerminalViewport, type ViewportRect } from "./native";

type Listener = (error: string | null) => void;

/** Which native item the terminal rectangle presents, under which native presentation epoch. */
export interface ViewportTarget {
  itemId: string | null;
  presentationEpoch: number;
  /** The terminal view is showing this item. */
  visible: boolean;
}

export interface ViewportTracker {
  /** Follow a new native presentation without tearing down tracking or any item. */
  setTarget(target: ViewportTarget): void;
  /** Stops tracking and hides the native surface (unless a newer tracker already took over). */
  dispose(): void;
}

interface ReportResult {
  /** Native applied the report for its current selection and epoch. */
  applied: boolean;
  sequence: number;
}

const NOT_APPLIED: ReportResult = { applied: false, sequence: 0 };
const sequenceStorageKey = "ultra9000.viewport.sequence";
let lastSequence = storedSequence();
// Last usable geometry. Hidden reports reuse it so native never resizes a PTY to zero.
let lastRect: ViewportRect = { x: 0, y: 0, width: 0, height: 0, topLeftRadius: 0, bottomLeftRadius: 0 };
let target: ViewportTarget = { itemId: null, presentationEpoch: 0, visible: false };
let reportedVisible = false;
let lastSentKey: string | null = null;
// Outcome of the latest report; a newer report replaces it.
let inFlight: Promise<ReportResult> = Promise.resolve(NOT_APPLIED);
let reportGeneration = 0;
let focusGeneration = 0;
// The current target still wants keyboard focus once its geometry is applied.
let focusWanted = false;
let errorListener: Listener | null = null;
// Only the most recently created tracker may report, so a late teardown cannot hide a restored view.
let activeTracker: symbol | null = null;
let remeasure: (() => void) | null = null;

function storedSequence(): number {
  try {
    const value = Number(sessionStorage.getItem(sequenceStorageKey));
    return Number.isSafeInteger(value) && value >= 0 ? value : 0;
  } catch {
    return 0;
  }
}

// Session storage preserves ordering through webview reloads and wall-clock rollback.
function nextSequence(): number {
  lastSequence = Math.max(lastSequence + 1, Date.now());
  try {
    sessionStorage.setItem(sequenceStorageKey, String(lastSequence));
  } catch {
    // Storage can be disabled; ordering within this live view remains monotonic.
  }
  return lastSequence;
}

function report(rect: ViewportRect, visible: boolean): void {
  const { itemId, presentationEpoch } = target;
  // Nothing to report without a selected item, or a hidden report before anything was ever shown:
  // native already hides the previous surface on its own presentation transitions.
  if (itemId === null || (!visible && (lastRect.width === 0 || lastRect.height === 0))) {
    if (lastSentKey !== null) {
      reportGeneration += 1;
      lastSentKey = null;
      inFlight = Promise.resolve(NOT_APPLIED);
    }
    if (reportedVisible) cancelTerminalFocus();
    reportedVisible = false;
    return;
  }

  const geometry = `${rect.x},${rect.y},${rect.width},${rect.height},${rect.topLeftRadius},${rect.bottomLeftRadius}`;
  const key = `${itemId}|${presentationEpoch}|${geometry}|${visible}`;
  if (key !== lastSentKey) {
    const generation = ++reportGeneration;
    lastSentKey = key;
    if (!visible && reportedVisible) cancelTerminalFocus();
    reportedVisible = visible;
    const sequence = nextSequence();
    inFlight = setTerminalViewport(itemId, presentationEpoch, rect, visible, sequence).then(
      (applied): ReportResult => {
        if (generation !== reportGeneration) return NOT_APPLIED;
        if (!applied) {
          // Native has moved to a newer presentation; its next state supersedes this report. Allow
          // a later measurement to resend rather than suppressing it as a duplicate.
          lastSentKey = null;
          return NOT_APPLIED;
        }
        errorListener?.(null);
        return { applied: true, sequence };
      },
      (error): ReportResult => {
        if (generation === reportGeneration) {
          lastSentKey = null;
          errorListener?.(`Terminal viewport update failed: ${errorMessage(error)}`);
        }
        return NOT_APPLIED;
      },
    );
  }
  if (reportedVisible && focusWanted) void runFocus(focusGeneration);
}

async function runFocus(generation: number): Promise<void> {
  for (;;) {
    const pending = inFlight;
    const result = await pending;
    if (generation !== focusGeneration || !focusWanted || !reportedVisible) return;
    // A newer layout may have arrived while the previous report was pending.
    if (pending !== inFlight) continue;
    // Not applied: keep wanting focus; the next successful report for this target retries.
    if (!result.applied) return;
    const { itemId, presentationEpoch } = target;
    if (itemId === null) return;
    focusWanted = false;
    try {
      // False means native moved on (stale epoch/sequence, hidden window); nothing to surface.
      await focusTerminal(itemId, presentationEpoch, result.sequence);
    } catch (error) {
      if (generation === focusGeneration && reportedVisible) {
        errorListener?.(`Cannot focus terminal: ${errorMessage(error)}`);
      }
    }
    return;
  }
}

/** Adopts a new target; true when the item, epoch or visibility changed. */
function applyTarget(next: ViewportTarget): boolean {
  const shown = next.visible && next.itemId !== null;
  const keyChanged = next.itemId !== target.itemId || next.presentationEpoch !== target.presentationEpoch;
  if (!keyChanged && shown === target.visible) return false;
  target = { itemId: next.itemId, presentationEpoch: next.presentationEpoch, visible: shown };
  // Work queued for any earlier presentation (A->B->A included) must never run for this one.
  cancelTerminalFocus();
  if (keyChanged) errorListener?.(null);
  // Each native presentation of an item's terminal takes keyboard focus once its geometry applies.
  focusWanted = shown;
  return true;
}

// Edges closer than this (CSS px) count as the same edge.
const FLUSH_EPSILON = 0.5;

/**
 * Radii for the native view's left corners, read from the host's container (the content panel) so
 * CSS stays the single source of truth. A corner is rounded only while the host sits flush in the
 * matching container corner (e.g. an exit bar above the host leaves its top-left square); a bordered
 * container never counts as flush, so native never masks a place the panel does not round.
 */
function cornerRadii(element: HTMLElement, bounds: DOMRect): Pick<ViewportRect, "topLeftRadius" | "bottomLeftRadius"> {
  const container = element.parentElement;
  if (container === null) return { topLeftRadius: 0, bottomLeftRadius: 0 };
  const outer = container.getBoundingClientRect();
  const flush = (a: number, b: number) => Math.abs(a - b) < FLUSH_EPSILON;
  if (!flush(bounds.left, outer.left)) return { topLeftRadius: 0, bottomLeftRadius: 0 };
  const style = getComputedStyle(container);
  // Computed corner radii are "<h>px" or "<h>px <v>px"; native masks circular arcs, so take the
  // smaller axis to stay inside the CSS corner.
  const radius = (value: string) => {
    const parts = value.split(" ").map((part) => Number.parseFloat(part));
    const smallest = Math.min(...parts);
    return Number.isFinite(smallest) && smallest > 0 ? smallest : 0;
  };
  return {
    topLeftRadius: flush(bounds.top, outer.top) ? radius(style.borderTopLeftRadius) : 0,
    bottomLeftRadius: flush(bounds.bottom, outer.bottom) ? radius(style.borderBottomLeftRadius) : 0,
  };
}

export function onViewportError(listener: Listener): () => void {
  errorListener = listener;
  return () => {
    if (errorListener === listener) errorListener = null;
  };
}

/**
 * Tracks the host element reserving the native terminal rectangle. Size and position changes are
 * coalesced to one report per animation frame; target changes report immediately. Zero-sized
 * geometry is reported as hidden with the last usable rect.
 */
export function trackViewport(element: HTMLElement, initialTarget: ViewportTarget): ViewportTracker {
  const token = Symbol("viewport-tracker");
  activeTracker = token;
  let frame = 0;
  let disposed = false;

  const live = () => !disposed && activeTracker === token;

  const measure = () => {
    if (frame !== 0) {
      cancelAnimationFrame(frame);
      frame = 0;
    }
    if (!live()) return;
    const bounds = element.getBoundingClientRect();
    // Round edges rather than sizes so adjacent layout boundaries stay consistent.
    const left = Math.max(0, Math.round(bounds.left));
    const top = Math.max(0, Math.round(bounds.top));
    const right = Math.max(left, Math.round(bounds.right));
    const bottom = Math.max(top, Math.round(bounds.bottom));
    const rect: ViewportRect = {
      x: left,
      y: top,
      width: right - left,
      height: bottom - top,
      ...cornerRadii(element, bounds),
    };
    const usable = rect.width > 0 && rect.height > 0;
    if (usable) lastRect = rect;
    // Window hiding is native-owned: the native view follows its window, not page visibility.
    report(lastRect, target.visible && usable);
  };
  remeasure = measure;

  const schedule = () => {
    if (frame === 0 && live()) frame = requestAnimationFrame(measure);
  };

  const observer = new ResizeObserver(schedule);
  observer.observe(element);
  // Position can change without the element's own size changing (e.g. surrounding layout).
  observer.observe(document.documentElement);
  window.addEventListener("resize", schedule);
  applyTarget(initialTarget);
  measure();

  return {
    setTarget(next) {
      if (live() && applyTarget(next)) measure();
    },
    dispose() {
      if (disposed) return;
      const wasActive = live();
      disposed = true;
      if (frame !== 0) cancelAnimationFrame(frame);
      frame = 0;
      observer.disconnect();
      window.removeEventListener("resize", schedule);
      if (wasActive) {
        activeTracker = null;
        remeasure = null;
        cancelTerminalFocus();
        report(lastRect, false);
      }
    },
  };
}

/**
 * Re-applies the current geometry and focuses the shown item once native accepts it (explicit
 * user return to the terminal, or the window becoming key again). Never spawns a process; no-op
 * unless the terminal view is showing an item.
 */
export function requestTerminalFocus(): void {
  if (!target.visible || remeasure === null) return;
  cancelTerminalFocus();
  focusWanted = true;
  lastSentKey = null;
  remeasure();
}

/** Drops queued focus work that no longer applies (window hidden, minimized or no longer key). */
export function cancelTerminalFocus(): void {
  focusGeneration += 1;
  focusWanted = false;
}
