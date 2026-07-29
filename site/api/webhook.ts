// POST /api/webhook  -> Stripe events. Mints and stores the license.
//
// The one rule that matters here: the signature is verified against the RAW body
// before anything parses it. This handler uses the Web signature precisely so
// that `await request.text()` IS the raw body. There is no body parser in front
// of it to defeat, which is the usual way this endpoint gets built wrong.

import type Stripe from 'stripe'
import { fail, guard, json, logFailure, methodNotAllowed, siteOrigin } from './_lib/http.js'
import { seedFromEnv } from './_lib/license.js'
import { sendLicenseEmail } from './_lib/mail.js'
import { NotPaidError, NotWardenSessionError, ensureLicense, getStripe } from './_lib/stripe.js'
import { issueToken } from './_lib/tokens.js'

/** checkout.session.completed fires as soon as checkout finishes, which for a
 *  delayed payment method is before the money has actually settled. The async
 *  event is the one that fires when it does, so both are handled and the
 *  payment_status check inside ensureLicense decides which one does the work. */
const FULFILLING_EVENTS = new Set(['checkout.session.completed', 'checkout.session.async_payment_succeeded'])

/** The key is already minted and stored by the time this runs, so a failure to
 *  build the download link must not throw: that would 500 a purchase that
 *  actually succeeded. The success page is the fallback, and it can always issue
 *  a fresh link itself. */
const downloadUrlFor = (sessionId: string): string => {
  const secret = process.env.DOWNLOAD_TOKEN_SECRET
  if (secret) {
    try {
      return `${siteOrigin()}/api/download?token=${encodeURIComponent(issueToken(sessionId, secret))}`
    } catch (err) {
      logFailure('webhook', err)
    }
  }
  return `${siteOrigin()}/success?session_id=${encodeURIComponent(sessionId)}`
}

export const handleWebhook = async (request: Request, stripe: Stripe): Promise<Response> => {
  const raw = await request.text()
  const signature = request.headers.get('stripe-signature')
  if (!signature) return fail(400, 'missing_signature')

  const secret = process.env.STRIPE_WEBHOOK_SECRET
  if (!secret) {
    logFailure('webhook', new Error('STRIPE_WEBHOOK_SECRET is not set'))
    return fail(500, 'not_configured')
  }

  let event: Stripe.Event
  try {
    // Verifies the v1 HMAC over `${timestamp}.${raw}` and enforces the timestamp
    // tolerance, so a body edited after signing and a stale capture both fail
    // here. The reason is never returned to the caller: a webhook that explains
    // why it rejected you is a signing oracle.
    event = stripe.webhooks.constructEvent(raw, signature, secret)
  } catch {
    return fail(400, 'invalid_signature')
  }

  if (!FULFILLING_EVENTS.has(event.type)) return json({ received: true, handled: false })

  const session = event.data.object as Stripe.Checkout.Session

  try {
    const fulfillment = await ensureLicense(stripe, session.id, seedFromEnv(process.env.LICENSE_SIGNING_KEY))

    // Only the call that actually minted sends mail. A Stripe retry lands here
    // with the key already stored, so the buyer does not get the same receipt
    // five times.
    if (fulfillment.minted) {
      await sendLicenseEmail({
        to: fulfillment.email,
        licenseKey: fulfillment.licenseKey,
        seats: fulfillment.seats,
        downloadUrl: downloadUrlFor(session.id),
      })
    }

    return json({ received: true, handled: true, minted: fulfillment.minted })
  } catch (err) {
    // Not paid yet is a normal state, not a failure: 200 so Stripe stops
    // retrying. Everything else throws on to a 500 so Stripe DOES retry, which
    // is the recovery path for a transient Stripe or network fault mid-mint.
    if (err instanceof NotPaidError) {
      return json({ received: true, handled: false, reason: 'not_paid' })
    }
    // Another product's checkout on the same Stripe account. Acknowledge it so
    // Stripe stops delivering, and never touch it.
    if (err instanceof NotWardenSessionError) {
      return json({ received: true, handled: false, reason: 'not_warden' })
    }
    throw err
  }
}

export const POST = (request: Request): Promise<Response> =>
  guard('webhook', () => handleWebhook(request, getStripe()))

export const GET = (): Response => methodNotAllowed('POST')
