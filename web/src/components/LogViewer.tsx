import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from 'react'
import type { LogLine } from '../api/client'

export interface LogViewerProps {
  /** Loads the full log history (C5 `GET /deployments/{uuid}` → `logs[]`). */
  fetchLines: () => Promise<LogLine[]>
  /** Live feed (C4 `deployment_log_appended`); returns an unsubscribe fn. */
  subscribe?: (onLine: (line: LogLine) => void) => () => void
  /** Bump to refetch (e.g. after a WS reconnect); overlap is deduped by ord. */
  refreshKey?: number
  height?: number
}

const ROW_HEIGHT = 22
/** Below this many rows we render everything; above, we window. */
const VIRTUAL_THRESHOLD = 400
const OVERSCAN = 30

function RowLine({ line, style }: { line: LogLine; style?: CSSProperties }) {
  const kindClass =
    line.kind === 'stderr'
      ? 'text-red-400'
      : line.kind === 'info'
        ? 'text-sky-300'
        : 'text-zinc-300'
  return (
    <div
      data-testid="log-line"
      data-ord={line.order}
      data-kind={line.kind}
      style={style}
      className={`flex gap-3 px-3 leading-[22px] whitespace-pre ${kindClass} ${
        line.hidden ? 'opacity-50' : ''
      }`}
    >
      <span className="w-20 shrink-0 select-none text-zinc-600">
        {new Date(line.timestamp).toLocaleTimeString('en-GB')}
      </span>
      <span>{line.content}</span>
    </div>
  )
}

/**
 * Deployment log viewer: fetch-on-load merged with live WS appends, deduped
 * by `order` and kept sorted; virtualized beyond {@link VIRTUAL_THRESHOLD}
 * rows; auto-scrolls while pinned to the bottom; hidden (internal) lines are
 * collapsed behind a toggle; stderr lines render red.
 */
export function LogViewer({ fetchLines, subscribe, refreshKey = 0, height = 480 }: LogViewerProps) {
  const [lines, setLines] = useState<LogLine[]>([])
  const [showHidden, setShowHidden] = useState(false)
  const [follow, setFollow] = useState(true)
  const [scrollTop, setScrollTop] = useState(0)
  const containerRef = useRef<HTMLDivElement>(null)

  const mergeLines = useCallback((incoming: LogLine[]) => {
    if (incoming.length === 0) return
    setLines((prev) => {
      const byOrd = new Map<number, LogLine>()
      for (const l of prev) byOrd.set(l.order, l)
      let changed = false
      for (const l of incoming) {
        if (!byOrd.has(l.order)) {
          byOrd.set(l.order, l)
          changed = true
        }
      }
      if (!changed) return prev
      return [...byOrd.values()].sort((a, b) => a.order - b.order)
    })
  }, [])

  useEffect(() => {
    let cancelled = false
    fetchLines()
      .then((fetched) => {
        if (!cancelled) mergeLines(fetched)
      })
      .catch(() => {
        // view keeps whatever it already has; caller surfaces load errors
      })
    return () => {
      cancelled = true
    }
  }, [fetchLines, refreshKey, mergeLines])

  useEffect(() => {
    if (!subscribe) return
    return subscribe((line) => mergeLines([line]))
  }, [subscribe, mergeLines])

  const visible = useMemo(
    () => (showHidden ? lines : lines.filter((l) => !l.hidden)),
    [lines, showHidden],
  )

  const hiddenCount = lines.length - lines.filter((l) => !l.hidden).length

  // pin-to-bottom auto-scroll
  useEffect(() => {
    if (!follow) return
    const el = containerRef.current
    if (el) el.scrollTop = el.scrollHeight
  }, [visible, follow])

  const onScroll = () => {
    const el = containerRef.current
    if (!el) return
    setScrollTop(el.scrollTop)
    const distanceFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight
    setFollow(distanceFromBottom < ROW_HEIGHT * 2)
  }

  const resumeFollow = () => {
    setFollow(true)
    const el = containerRef.current
    if (el) el.scrollTop = el.scrollHeight
  }

  const virtual = visible.length > VIRTUAL_THRESHOLD
  let start = 0
  let end = visible.length
  if (virtual) {
    start = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN)
    end = Math.min(visible.length, Math.ceil((scrollTop + height) / ROW_HEIGHT) + OVERSCAN)
  }
  const slice = visible.slice(start, end)

  return (
    <div className="overflow-hidden rounded-lg border border-zinc-800 bg-zinc-950">
      <div className="flex items-center justify-between border-b border-zinc-800 bg-zinc-900/60 px-3 py-1.5 text-xs text-zinc-400">
        <label className="flex cursor-pointer items-center gap-1.5">
          <input
            type="checkbox"
            checked={showHidden}
            onChange={(e) => setShowHidden(e.target.checked)}
            className="accent-zinc-400"
          />
          Show hidden {hiddenCount > 0 ? `(${hiddenCount})` : ''}
        </label>
        <div className="flex items-center gap-3">
          <span>{visible.length} lines</span>
          {!follow && (
            <button
              type="button"
              onClick={resumeFollow}
              className="rounded border border-zinc-700 px-2 py-0.5 text-zinc-300 hover:bg-zinc-800"
            >
              Follow
            </button>
          )}
        </div>
      </div>
      <div
        ref={containerRef}
        onScroll={onScroll}
        role="log"
        aria-label="deployment logs"
        style={{ height }}
        className="overflow-auto py-1 font-mono text-xs"
      >
        {visible.length === 0 ? (
          <div className="px-3 py-4 text-zinc-500">Waiting for logs…</div>
        ) : virtual ? (
          <div style={{ height: visible.length * ROW_HEIGHT, position: 'relative' }}>
            {slice.map((l, i) => (
              <RowLine
                key={l.order}
                line={l}
                style={{
                  position: 'absolute',
                  top: (start + i) * ROW_HEIGHT,
                  left: 0,
                  right: 0,
                  height: ROW_HEIGHT,
                }}
              />
            ))}
          </div>
        ) : (
          slice.map((l) => <RowLine key={l.order} line={l} />)
        )}
      </div>
    </div>
  )
}
