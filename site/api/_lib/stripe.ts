// The SKU table and the fulfillment core. Stripe is the system of record: there
// is no database, so a session's metadata IS the license record (LAUNCH-SPEC
// section 1).

import Stripe from 'stripe'
import { mintKey, publicKeyFromSeed, verifyKey, type LicenseClaims } from './license.js'

/** Pinned to the version this SDK generation was built against, so a Stripe
 *  account-level API upgrade cannot change response shapes under us. */
export const API_VERSION = '2026-06-24.dahlia'
export const CURRENCY = 'usd'
export const STATEMENT_DESCRIPTOR_SUFFIX = 'WARDEN'

export type SkuId = 'warden-1' | 'warden-2' | 'warden-3'

export interface Sku {
  id: SkuId
  name: string
  description: string
  /** Minor units. Reference data only: it is what scripts/stripe-provision.mjs
   *  creates a Price with in a FRESH account, and what the tests assert against.
   *  Checkout never sends an amount, so this value cannot drift into a charge. */
  amount: number
  seats: number
  /** The live objects in acct_1TxtpfEnDphWh4zv (LAUNCH-SPEC section 0). Defaults,
   *  overridable per SKU by env, so pointing the site at a different Stripe
   *  account is config rather than a code change. */
  priceId: string
  productId: string
}

/**
 * The SKU table from LAUNCH-SPEC section 0. A client never supplies a price; it
 * names a SKU and the server resolves it here.
 *
 * Names, descriptions and metadata mirror the live Stripe objects exactly, so
 * re-running the provisioning script against this account is a no-op rather
 * than a rewrite of copy that is already on the checkout page.
 */
export const SKUS: Record<SkuId, Sku> = {
  'warden-1': {
    id: 'warden-1',
    name: 'WARDEN, one Mac',
    description:
      'WARDEN on a single Mac. A live board for every coding agent running on your machine. One payment, yours forever.',
    amount: 1500,
    seats: 1,
    priceId: 'price_1TycKNEnDphWh4zvWeScnLkf',
    productId: 'prod_UyZTENqYDCUtrt',
  },
  'warden-2': {
    id: 'warden-2',
    name: 'WARDEN, two Macs',
    description:
      'WARDEN on two Macs. A live board for every coding agent running on your machines. One payment, yours forever.',
    amount: 2500,
    seats: 2,
    priceId: 'price_1TycKSEnDphWh4zvvE85TJf5',
    productId: 'prod_UyZTw8O7rsyQjk',
  },
  'warden-3': {
    id: 'warden-3',
    name: 'WARDEN, three Macs',
    description:
      'WARDEN on three Macs. A live board for every coding agent running on your machines. One payment, yours forever.',
    amount: 3000,
    seats: 3,
    priceId: 'price_1TycKUEnDphWh4zvNSxOn2yI',
    productId: 'prod_UyZToa7Y1nPVMG',
  },
}

export const SKU_IDS = Object.keys(SKUS) as SkuId[]

export const isSkuId = (value: unknown): value is SkuId =>
  typeof value === 'string' && Object.prototype.hasOwnProperty.call(SKUS, value)

/** PRICE_WARDEN_1 / _2 / _3 override the committed defaults. */
export const priceEnvVar = (sku: Sku): string => `PRICE_${sku.id.toUpperCase().replace(/-/g, '_')}`

/** The Price the Checkout Session charges. Stripe holds the amount; nothing here
 *  restates it, so the site and the dashboard cannot disagree about what a
 *  buyer paid. */
export const priceIdFor = (sku: Sku): string => process.env[priceEnvVar(sku)]?.trim() || sku.priceId

let client: Stripe | null = null

export const getStripe = (): Stripe => {
  const key = process.env.STRIPE_SECRET_KEY
  if (!key) throw new Error('STRIPE_SECRET_KEY is not set')
  if (!client) client = new Stripe(key, { apiVersion: API_VERSION as Stripe.LatestApiVersion })
  return client
}

export class NotPaidError extends Error {}
export class UnknownSessionError extends Error {}

/** The session belongs to another product in the same Stripe account. Launch
 *  runs on the shared FRONTIER account (LAUNCH-SPEC section 1), so the webhook
 *  receives that business's checkout events too. Those are not failures and must
 *  not be retried: a 500 here would put Stripe into a permanent retry loop
 *  against traffic that has nothing to do with WARDEN. */
export class NotWardenSessionError extends Error {}

export interface Fulfillment {
  licenseKey: string
  email: string
  seats: number
  iat: number
  /** True when this call is the one that created the key. Tests and the webhook
   *  use it to prove a replay did not mint twice. */
  minted: boolean
}

/**
 * Returns the license for a paid Checkout Session, minting and storing it on
 * first call. Idempotent by construction, in two layers:
 *
 *  1. It reads the session back from Stripe and returns any key already stored.
 *  2. `iat` is the session's own `created` timestamp, not `Date.now()`, so the
 *     minted key is a pure function of (session id, email, seats). Two webhook
 *     retries racing each other therefore write the SAME string. The lost
 *     update is a no-op instead of a second, different license.
 *
 * Seats come from the SKU table via `metadata.sku`, never from `metadata.seats`,
 * so stamping a bigger number into metadata buys nothing.
 */
export const ensureLicense = async (
  stripe: Stripe,
  sessionId: string,
  seed: Buffer,
): Promise<Fulfillment> => {
  const session = await stripe.checkout.sessions.retrieve(sessionId)
  if (!session || typeof session.id !== 'string') throw new UnknownSessionError('no such session')
  if (session.payment_status !== 'paid') throw new NotPaidError(`payment_status=${session.payment_status}`)

  const sku = session.metadata?.sku
  if (!isSkuId(sku)) throw new NotWardenSessionError('session has no recognised warden sku')
  const seats = SKUS[sku].seats

  const rawEmail = session.customer_details?.email ?? session.customer_email
  if (!rawEmail) throw new Error('session has no customer email')
  const email = rawEmail.trim().toLowerCase()

  const iat = typeof session.created === 'number' ? session.created : Math.floor(Date.now() / 1000)
  const claims: LicenseClaims = { v: 1, id: session.id, email, seats, iat }

  const stored = session.metadata?.license_key
  if (stored && verifyKey(stored, publicKeyFromSeed(seed))) {
    return { licenseKey: stored, email, seats, iat, minted: false }
  }

  // Either no key yet, or one this signing identity cannot verify (a rotated
  // key leaves those behind). Re-minting is the self-heal: the buyer's session
  // is the record, and it should always hold a key the shipped app accepts.
  const licenseKey = mintKey(seed, claims)

  await stripe.checkout.sessions.update(session.id, {
    metadata: { ...(session.metadata ?? {}), license_key: licenseKey, license_iat: String(iat), license_seats: String(seats) },
  })

  // The PaymentIntent is what a refund or a dispute is opened against, so the
  // key is written there too: support should never have to join two objects.
  const paymentIntentId = typeof session.payment_intent === 'string' ? session.payment_intent : session.payment_intent?.id
  if (paymentIntentId) {
    await stripe.paymentIntents.update(paymentIntentId, {
      metadata: { license_key: licenseKey, warden_sku: sku, license_seats: String(seats) },
    })
  }

  return { licenseKey, email, seats, iat, minted: true }
}
