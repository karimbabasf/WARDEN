// POST /api/checkout  { sku } -> { url }
//
// The price is never in the request. The client names a SKU; the server reads
// the amount out of the SKUS table (LAUNCH-SPEC section 6).

import type Stripe from 'stripe'
import { fail, guard, json, methodNotAllowed, readJson, siteOrigin } from './_lib/http.js'
import { SKUS, STATEMENT_DESCRIPTOR_SUFFIX, getStripe, isSkuId, priceIdFor } from './_lib/stripe.js'

/** Any of these in the body means the caller is trying to set the price, name
 *  the product, or pick the quantity. There is no charitable reading of that, so
 *  it is a 400 rather than a silent ignore: failing loudly is what makes the
 *  rule testable and keeps a future refactor from quietly starting to read one. */
const CLIENT_PRICE_FIELDS = [
  'amount',
  'price',
  'price_data',
  'unit_amount',
  'unit_amount_decimal',
  'currency',
  'quantity',
  'seats',
  'line_items',
  'total',
  'discount',
  'coupon',
]

export const handleCheckout = async (request: Request, stripe: Stripe): Promise<Response> => {
  const body = await readJson(request)
  if (!body) return fail(400, 'invalid_json')

  for (const field of CLIENT_PRICE_FIELDS) {
    if (Object.prototype.hasOwnProperty.call(body, field)) return fail(400, 'price_is_server_side')
  }

  if (!isSkuId(body.sku)) return fail(400, 'unknown_sku')
  const sku = SKUS[body.sku]
  const origin = siteOrigin()

  const session = await stripe.checkout.sessions.create({
    mode: 'payment',
    // A Price id, never an amount. Stripe is the single source of truth for what
    // this costs, so the site cannot drift from what the buyer is actually
    // charged. A bad PRICE_WARDEN_* fails the create call and 500s: no session
    // is made, so a misconfiguration cannot quietly charge the wrong price.
    line_items: [{ price: priceIdFor(sku), quantity: 1 }],
    // One-time payments do not create a Customer by default, and /api/recover
    // finds a buyer's past purchases by customer email. Without this, key
    // recovery has nothing to search.
    customer_creation: 'always',
    payment_intent_data: {
      statement_descriptor_suffix: STATEMENT_DESCRIPTOR_SUFFIX,
      metadata: { warden_sku: sku.id },
    },
    success_url: `${origin}/success?session_id={CHECKOUT_SESSION_ID}`,
    cancel_url: `${origin}/#pricing`,
    // seats is written for the dashboard only. Fulfillment re-derives it from
    // sku through the server table and never reads this field.
    metadata: { sku: sku.id, seats: String(sku.seats) },
  })

  if (!session.url) return fail(502, 'checkout_unavailable')
  return json({ url: session.url })
}

export const POST = (request: Request): Promise<Response> =>
  guard('checkout', () => handleCheckout(request, getStripe()))

export const GET = (): Response => methodNotAllowed('POST')
