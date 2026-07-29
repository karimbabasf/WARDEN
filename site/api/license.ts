// GET /api/license?session_id=  -> { email, seats, licenseKey, downloadUrl, expiresAt }
//
// What the success page calls. Only a session Stripe reports as paid gets a key.

import type Stripe from 'stripe'
import { fail, guard, json, methodNotAllowed, siteOrigin } from './_lib/http.js'
import { seedFromEnv } from './_lib/license.js'
import {
  NotPaidError,
  NotWardenSessionError,
  UnknownSessionError,
  ensureLicense,
  getStripe,
} from './_lib/stripe.js'
import { TOKEN_TTL_SECONDS, issueToken } from './_lib/tokens.js'

const SESSION_ID = /^[A-Za-z0-9_]+$/

const isMissingResource = (err: unknown): boolean =>
  typeof err === 'object' &&
  err !== null &&
  ((err as { code?: string }).code === 'resource_missing' || (err as { statusCode?: number }).statusCode === 404)

export const handleLicense = async (request: Request, stripe: Stripe): Promise<Response> => {
  const sessionId = new URL(request.url).searchParams.get('session_id')
  if (!sessionId || !SESSION_ID.test(sessionId)) return fail(400, 'invalid_session_id')

  const downloadSecret = process.env.DOWNLOAD_TOKEN_SECRET
  if (!downloadSecret) return fail(500, 'not_configured')

  try {
    // Mints on the spot if the webhook has not landed yet. The success page
    // redirect regularly beats the webhook, and "your key is not ready, refresh"
    // is a broken product. Because iat comes from the session, the key minted
    // here is byte-identical to the one the webhook would have minted, so the
    // race has no losing side.
    const fulfillment = await ensureLicense(stripe, sessionId, seedFromEnv(process.env.LICENSE_SIGNING_KEY))

    const now = Math.floor(Date.now() / 1000)
    const token = issueToken(sessionId, downloadSecret, now)

    return json({
      email: fulfillment.email,
      seats: fulfillment.seats,
      licenseKey: fulfillment.licenseKey,
      downloadUrl: `${siteOrigin()}/api/download?token=${encodeURIComponent(token)}`,
      expiresAt: new Date((now + TOKEN_TTL_SECONDS) * 1000).toISOString(),
    })
  } catch (err) {
    if (err instanceof NotPaidError) return fail(409, 'payment_not_complete')
    // A session for another product on the shared account is simply not a
    // WARDEN license, and it is not this endpoint's job to explain that.
    if (err instanceof NotWardenSessionError) return fail(404, 'not_found')
    if (err instanceof UnknownSessionError || isMissingResource(err)) return fail(404, 'not_found')
    throw err
  }
}

export const GET = (request: Request): Promise<Response> =>
  guard('license', () => handleLicense(request, getStripe()))

export const POST = (): Response => methodNotAllowed('GET')
