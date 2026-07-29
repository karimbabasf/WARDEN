// Response helpers. Two rules live here: /api/* never returns a stack trace, and
// the site's own origin is never taken from a request header.

export const json = (body: unknown, status = 200, headers: HeadersInit = {}): Response =>
  new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json; charset=utf-8', 'cache-control': 'no-store', ...headers },
  })

/** The only error shape a client ever sees: a stable machine code, no detail.
 *  Anything worth knowing is in the server log, which the client cannot read. */
export const fail = (status: number, code: string): Response => json({ error: code }, status)

export const methodNotAllowed = (allow: string): Response =>
  json({ error: 'method_not_allowed' }, 405, { allow })

/**
 * The site's absolute origin, for success_url / cancel_url / download links.
 *
 * Deliberately NOT derived from the Host header. A poisoned Host would put an
 * attacker's domain in success_url, and since that URL carries
 * {CHECKOUT_SESSION_ID}, the attacker would receive the session id of a purchase
 * someone else paid for and could then read their license key from /api/license.
 * Every source below is set by the platform, not by the caller.
 */
let warnedAboutOrigin = false

export const siteOrigin = (): string => {
  const configured = process.env.PUBLIC_SITE_URL?.trim()
  if (configured) return configured.replace(/\/+$/, '')

  // The fallbacks are correct but not necessarily the domain we want to live
  // on: this origin is baked into emailed download links, which outlive the
  // deployment that sent them. Say so once per cold start, at deploy time,
  // rather than letting a customer find it in a dead link weeks later.
  if (!warnedAboutOrigin) {
    warnedAboutOrigin = true
    console.warn('[http] PUBLIC_SITE_URL is not set, falling back to the Vercel-provided domain')
  }

  // Stable across deployments (it is the project's production domain), which is
  // why it is preferred over VERCEL_URL, the per-deployment hostname.
  const production = process.env.VERCEL_PROJECT_PRODUCTION_URL?.trim()
  if (production) return `https://${production}`

  const deployment = process.env.VERCEL_URL?.trim()
  if (deployment) return `https://${deployment}`

  return 'http://localhost:3000'
}

/** Logs an unexpected failure without leaking its detail to the caller. The
 *  message is kept (it is ours, not the customer's) but never the stack, and
 *  never the request body. */
export const logFailure = (scope: string, err: unknown): void => {
  const message = err instanceof Error ? err.message : 'unknown error'
  console.error(`[${scope}] ${message}`)
}

/** Wraps a handler so no throw escapes as a 500 with a stack in the body. */
export const guard = async (scope: string, run: () => Promise<Response>): Promise<Response> => {
  try {
    return await run()
  } catch (err) {
    logFailure(scope, err)
    return fail(500, 'internal_error')
  }
}

export const readJson = async (request: Request): Promise<Record<string, unknown> | null> => {
  try {
    const parsed: unknown = await request.json()
    if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) return null
    return parsed as Record<string, unknown>
  } catch {
    return null
  }
}
