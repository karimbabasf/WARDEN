// A Stripe stand-in for the handler tests.
//
// `webhooks` is deliberately the REAL implementation from the SDK: signature
// verification is the property under test, so faking it would test nothing. Only
// the network-backed resources are stubbed.

import Stripe from 'stripe'
import { API_VERSION } from '../_lib/stripe.js'

const real = new Stripe('sk_test_offline_signature_only', { apiVersion: API_VERSION as Stripe.LatestApiVersion })

export const webhooks = real.webhooks

export interface SeedSession {
  id: string
  payment_status?: string
  metadata?: Record<string, string>
  email?: string | null
  created?: number
  payment_intent?: string | null
  customer?: string | null
}

export interface FakeCalls {
  sessionCreate: unknown[]
  sessionUpdate: { id: string; params: unknown }[]
  sessionRetrieve: string[]
  paymentIntentUpdate: { id: string; params: unknown }[]
  priceRetrieve: string[]
  customerList: unknown[]
  sessionList: unknown[]
}

const toSession = (seed: SeedSession): {
  id: string
  object: string
  payment_status: string
  status: string
  created: number
  customer: string | null
  customer_email: string | null
  customer_details: { email: string | null }
  payment_intent: string | null
  metadata: Record<string, string>
  url: string
} => ({
  id: seed.id,
  object: 'checkout.session',
  payment_status: seed.payment_status ?? 'paid',
  status: 'complete',
  created: seed.created ?? 1785700000,
  customer: seed.customer ?? null,
  customer_email: null,
  customer_details: { email: seed.email === undefined ? 'Buyer@Example.com' : seed.email },
  payment_intent: seed.payment_intent === undefined ? 'pi_test_1' : seed.payment_intent,
  metadata: { sku: 'warden-1', seats: '1', ...(seed.metadata ?? {}) },
  url: `https://checkout.stripe.com/c/pay/${seed.id}`,
})

export const createFakeStripe = (seeds: SeedSession[] = []) => {
  const sessions = new Map<string, ReturnType<typeof toSession>>()
  for (const seed of seeds) sessions.set(seed.id, toSession(seed))

  const calls: FakeCalls = {
    sessionCreate: [],
    sessionUpdate: [],
    sessionRetrieve: [],
    paymentIntentUpdate: [],
    priceRetrieve: [],
    customerList: [],
    sessionList: [],
  }

  let customers: { id: string; email: string }[] = []
  let sessionsByCustomer: Record<string, string[]> = {}
  let prices: Record<string, { id: string; unit_amount: number | null; currency: string; active: boolean }> = {}
  let createdCounter = 0

  const fake = {
    calls,
    sessions,
    seed(session: SeedSession) {
      sessions.set(session.id, toSession(session))
      return this
    },
    withCustomers(list: { id: string; email: string }[], owned: Record<string, string[]>) {
      customers = list
      sessionsByCustomer = owned
      return this
    },
    withPrice(id: string, unit_amount: number | null, currency = 'usd', active = true) {
      prices[id] = { id, unit_amount, currency, active }
      return this
    },
    checkout: {
      sessions: {
        async create(params: unknown) {
          calls.sessionCreate.push(params)
          createdCounter += 1
          const id = `cs_test_created_${createdCounter}`
          return { id, url: `https://checkout.stripe.com/c/pay/${id}` }
        },
        async retrieve(id: string) {
          calls.sessionRetrieve.push(id)
          const session = sessions.get(id)
          if (!session) {
            const err = new Error('No such checkout session') as Error & { code: string; statusCode: number }
            err.code = 'resource_missing'
            err.statusCode = 404
            throw err
          }
          // A fresh copy each time, like a real round trip: a handler that
          // mutates the object it got back must not appear idempotent by
          // accident.
          return JSON.parse(JSON.stringify(session))
        },
        async update(id: string, params: { metadata?: Record<string, string> }) {
          calls.sessionUpdate.push({ id, params })
          const session = sessions.get(id)
          if (!session) throw new Error('No such checkout session')
          session.metadata = { ...session.metadata, ...(params.metadata ?? {}) }
          return JSON.parse(JSON.stringify(session))
        },
        async list(params: { customer?: string; limit?: number }) {
          calls.sessionList.push(params)
          const ids = params.customer ? (sessionsByCustomer[params.customer] ?? []) : []
          return { data: ids.map((id) => JSON.parse(JSON.stringify(sessions.get(id)))).filter(Boolean) }
        },
      },
    },
    paymentIntents: {
      async update(id: string, params: unknown) {
        calls.paymentIntentUpdate.push({ id, params })
        return { id }
      },
    },
    prices: {
      async retrieve(id: string) {
        calls.priceRetrieve.push(id)
        const price = prices[id]
        if (!price) {
          const err = new Error('No such price') as Error & { code: string }
          err.code = 'resource_missing'
          throw err
        }
        return price
      },
    },
    customers: {
      async list(params: { email?: string }) {
        calls.customerList.push(params)
        return { data: customers.filter((c) => c.email === params.email) }
      },
    },
    webhooks,
  }

  return fake
}

export type FakeStripe = ReturnType<typeof createFakeStripe>

/** The handlers take a real Stripe type; the fake implements only the surface
 *  they touch. One cast, in one place, instead of a cast at every call site. */
export const asStripe = (fake: FakeStripe): Stripe => fake as unknown as Stripe

export const post = (url: string, body: unknown, headers: HeadersInit = {}): Request =>
  new Request(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json', ...headers },
    body: typeof body === 'string' ? body : JSON.stringify(body),
  })

export const get = (url: string): Request => new Request(url, { method: 'GET' })

/** Signs a payload the way Stripe does, so constructEvent runs its real check. */
export const signedRequest = (
  url: string,
  payload: string,
  secret: string,
  timestamp = Math.floor(Date.now() / 1000),
): Request =>
  post(url, payload, {
    'stripe-signature': webhooks.generateTestHeaderString({ payload, secret, timestamp }),
  })

export const checkoutCompletedEvent = (sessionId: string, id = 'evt_test_1'): string =>
  JSON.stringify({
    id,
    object: 'event',
    api_version: API_VERSION,
    created: 1785700000,
    type: 'checkout.session.completed',
    data: { object: { id: sessionId, object: 'checkout.session' } },
  })
