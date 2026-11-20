import { Component, Suspense, type ErrorInfo, type ReactNode } from 'react'
import { Loader2 } from 'lucide-react'
import i18n from '@/i18n'

interface BoundaryProps {
  children: ReactNode
  /** A new value clears a caught error (the route changed). */
  resetKey: string
}

/**
 * Catches render errors and failed page-chunk loads (server restarted or updated while the tab stayed open) so the
 * user sees a message with a reload button instead of a blank page. A rejected `lazy()` import stays rejected, so
 * reloading is the way out.
 */
class Boundary extends Component<BoundaryProps, { failed: boolean }> {
  state = { failed: false }

  static getDerivedStateFromError() {
    return { failed: true }
  }

  componentDidUpdate(prev: BoundaryProps) {
    if (this.state.failed && prev.resetKey !== this.props.resetKey) this.setState({ failed: false })
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    console.error('page failed to load', error, info.componentStack)
  }

  render() {
    if (!this.state.failed) return this.props.children
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 p-6 text-center" role="alert" data-testid="route-error">
        <p className="max-w-md text-sm text-muted">{i18n.t('common.pageFailed') as string}</p>
        <button type="button" className="btn" onClick={() => window.location.reload()}>
          {i18n.t('common.reload') as string}
        </button>
      </div>
    )
  }
}

/** Same spinner as the in-page loading states, but silent: no text or live region while a page chunk streams in. */
function Fallback() {
  return (
    <div className="flex h-full items-center justify-center text-muted" aria-hidden="true" data-testid="route-loading">
      <Loader2 className="animate-spin" size={18} />
    </div>
  )
}

/** Wraps the routes: lazy pages show the fallback while loading and an error with a reload button if that fails. */
export function RouteShell({ resetKey, children }: { resetKey: string; children: ReactNode }) {
  return (
    <Boundary resetKey={resetKey}>
      <Suspense fallback={<Fallback />}>{children}</Suspense>
    </Boundary>
  )
}
