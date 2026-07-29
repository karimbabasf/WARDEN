// POST /api/recover  { email } -> { ok: true }, always.
//
// Re-sends a buyer's key. The response is identical whether or not that address
// ever bought anything: this endpoint is unauthenticated, so any difference in
// status, body, or error would turn it into a "does this person use WARDEN"
// oracle for anyone with a list of email addresses.

import { createHash } from 'node:crypto'
import type Stripe from 'stripe'
import { fail, guard, json, methodNotAllowed, readJson, siteOrigin } from './_lib/http.js'
import { seedFromEnv } from './_lib/license.js'
import { sendLicenseEmail } from './_lib/mail.js'
import { NotPaidError, NotWardenSessionError, ensureLicense, getStripe } from './_lib/stripe.js'
import { issueToken } from './_lib/tokens.js'

const WINDOW_MS = 15 * 60 * 1000
const MAX_PER_IP = 5
const MAX_PER_EMAIL = 3
/** Bounds the limiter's own memory. Without a cap, a flood of unique keys is a
 *  way to grow the map until the function dies. */
const MAX_TRACKED = 5000
const MAX_EMAIL_LENGTH = 320
const MAX_KEYS_RESENT = 3
const EMAIL_SHAPE = /^[^\s@]+@[^\s@.]+(\.[^\s@.]+)+$/

interface Hit {
  count: number
  resetAt: number
}

// Per-instance and deliberately so: LAUNCH-SPEC forbids a database or KV, and a
// serverless instance is short-lived, so this is a speed bump rather than a
// guarantee. It stops the trivial scripted flood, which is the actual threat to
// an endpoint that sends mail. Stripe's own rate limits are the backstop.
const hits = new Map<string, Hit>()

const rateLimit = (key: string, max: number, now: number): boolean => {
  if (hits.size > MAX_TRACKED) {
    for (const [k, hit] of hits) if (hit.resetAt <= now) hits.delete(k)
    if (hits.size > MAX_TRACKED) hits.clear()
  }

  const hit = hits.get(key)
  if (!hit || hit.resetAt <= now) {
    hits.set(key, { count: 1, resetAt: now + WINDOW_MS })
    return true
  }
  hit.count += 1
  return hit.count <= max
}

/** Hashed so the limiter never holds a customer's address in memory longer than
 *  the request that carried it. */
const emailKey = (email: string): string => `e:${createHash('sha256').update(email).digest('base64url')}`

const clientIp = (request: Request): string => {
  const header =
    request.headers.get('x-vercel-forwarded-for') ??
    request.headers.get('x-forwarded-for') ??
    request.headers.get('x-real-ip') ??
    'unknown'
  return `i:${(header.split(',')[0] ?? 'unknown').trim()}`
}

/** Stripe's email filter is case-sensitive, so a buyer who typed Mixed.Case at
 *  checkout is not found by the lowercased form alone. Both are queried. */
const findCustomers = async (stripe: Stripe, email: string, raw: string): Promise<string[]> => {
  const queries = raw !== email ? [email, raw] : [email]
  const ids = new Set<string>()
  for (const query of queries) {
    const page = await stripe.customers.list({ email: query, limit: 10 })
    for (const customer of page.data) ids.add(customer.id)
  }
  return [...ids]
}

export const handleRecover = async (
  request: Request,
  stripe: Stripe,
  now: number = Date.now(),
): Promise<Response> => {
  const body = await readJson(request)
  if (!body) return fail(400, 'invalid_json')

  const raw = typeof body.email === 'string' ? body.email.trim() : ''
  const email = raw.toLowerCase()

  // A malformed address is rejected before any Stripe call, but with the same
  // shape as success would take, so this is not a probe for valid formats.
  if (!raw || raw.length > MAX_EMAIL_LENGTH || !EMAIL_SHAPE.test(raw)) return json({ ok: true })

  if (!rateLimit(clientIp(request), MAX_PER_IP, now)) return fail(429, 'rate_limited')
  // The per-address limit is silent: a 429 here would say "someone asked for
  // this address recently", which is exactly the fact being protected.
  if (!rateLimit(emailKey(email), MAX_PER_EMAIL, now)) return json({ ok: true })

  try {
    const seed = seedFromEnv(process.env.LICENSE_SIGNING_KEY)
    const downloadSecret = process.env.DOWNLOAD_TOKEN_SECRET
    const customers = await findCustomers(stripe, email, raw)

    const sent = new Set<string>()
    for (const customer of customers) {
      const sessions = await stripe.checkout.sessions.list({ customer, limit: 100 })
      for (const session of sessions.data) {
        if (sent.size >= MAX_KEYS_RESENT) break
        if (session.payment_status !== 'paid') continue

        let fulfillment
        try {
          // Mints if the webhook never landed for this session, so recovery
          // also repairs a purchase that silently never got a key.
          fulfillment = await ensureLicense(stripe, session.id, seed)
        } catch (err) {
          // Not paid, or a purchase of another product on the shared account.
          // Both are ordinary for this customer: skip the session, keep looking
          // at the rest, or a FRONTIER order would abort a WARDEN recovery.
          if (err instanceof NotPaidError || err instanceof NotWardenSessionError) continue
          throw err
        }
        if (sent.has(fulfillment.licenseKey)) continue
        sent.add(fulfillment.licenseKey)

        await sendLicenseEmail({
          to: fulfillment.email,
          licenseKey: fulfillment.licenseKey,
          seats: fulfillment.seats,
          downloadUrl: downloadSecret
            ? `${siteOrigin()}/api/download?token=${encodeURIComponent(issueToken(session.id, downloadSecret))}`
            : `${siteOrigin()}/success?session_id=${encodeURIComponent(session.id)}`,
        })
      }
    }
  } catch (err) {
    // Swallowed on purpose. A 500 here would tell the caller that the lookup got
    // far enough to fail, which is more than a stranger should learn. The
    // failure is in the log, where support can see it.
    console.error(`[recover] lookup failed: ${err instanceof Error ? err.message : 'unknown error'}`)
  }

  return json({ ok: true })
}

export const POST = (request: Request): Promise<Response> =>
  guard('recover', () => handleRecover(request, getStripe()))

export const GET = (): Response => methodNotAllowed('POST')

/** Test seam: the limiter is module state, so a suite that does not reset it
 *  leaks counts between cases. */
export const __resetRateLimit = (): void => hits.clear()
