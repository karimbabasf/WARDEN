import './env.js'
import { beforeEach, describe, expect, it } from 'vitest'
import { GET, handleCheckout } from '../checkout.js'
import { SKUS, priceIdFor } from '../_lib/stripe.js'
import { asStripe, createFakeStripe, post } from './fake-stripe.js'

const URL_ = 'https://warden.test/api/checkout'
const body = async (response: Response) => (await response.json()) as Record<string, unknown>

const PRICE_ENVS = ['PRICE_WARDEN_1', 'PRICE_WARDEN_2', 'PRICE_WARDEN_3']

beforeEach(() => {
  for (const name of PRICE_ENVS) delete process.env[name]
})

describe('the price is server-side, always', () => {
  it.each([
    { amount: 1 },
    { price: 1 },
    { unit_amount: 1 },
    { unit_amount_decimal: '1' },
    { currency: 'usd' },
    { quantity: 99 },
    { seats: 99 },
    { line_items: [] },
    { price_data: {} },
    { total: 0 },
    { discount: 100 },
    { coupon: 'FREE' },
  ])('rejects a body carrying %o with a 400', async (extra) => {
    const fake = createFakeStripe()
    const response = await handleCheckout(post(URL_, { sku: 'warden-1', ...extra }), asStripe(fake))

    expect(response.status).toBe(400)
    expect(await body(response)).toEqual({ error: 'price_is_server_side' })
    expect(fake.calls.sessionCreate).toHaveLength(0)
  })

  it('rejects a client amount even when it is the correct one', async () => {
    const fake = createFakeStripe()
    const response = await handleCheckout(post(URL_, { sku: 'warden-1', amount: 1500 }), asStripe(fake))
    expect(response.status).toBe(400)
    expect(fake.calls.sessionCreate).toHaveLength(0)
  })

  it.each(Object.values(SKUS))('charges the table Price id for $id, not anything from the request', async (sku) => {
    const fake = createFakeStripe()
    const response = await handleCheckout(post(URL_, { sku: sku.id }), asStripe(fake))

    expect(response.status).toBe(200)
    const params = fake.calls.sessionCreate[0] as {
      line_items: { price: string; quantity: number; price_data?: unknown }[]
      metadata: Record<string, string>
    }
    expect(params.line_items[0]?.price).toBe(sku.priceId)
    expect(params.line_items[0]?.quantity).toBe(1)
    // No amount is ever sent: Stripe owns what this costs.
    expect(params.line_items[0]?.price_data).toBeUndefined()
    expect(params.metadata.sku).toBe(sku.id)
  })

  it('never sends an amount or a currency to Stripe', async () => {
    const fake = createFakeStripe()
    await handleCheckout(post(URL_, { sku: 'warden-3' }), asStripe(fake))

    const serialised = JSON.stringify(fake.calls.sessionCreate[0])
    expect(serialised).not.toContain('unit_amount')
    expect(serialised).not.toContain('price_data')
    expect(serialised).not.toContain('3000')
  })

  it('carries the live ids and prices from LAUNCH-SPEC section 0', () => {
    expect([SKUS['warden-1'].amount, SKUS['warden-2'].amount, SKUS['warden-3'].amount]).toEqual([1500, 2500, 3000])
    expect([SKUS['warden-1'].seats, SKUS['warden-2'].seats, SKUS['warden-3'].seats]).toEqual([1, 2, 3])
    expect(SKUS['warden-1'].priceId).toBe('price_1TycKNEnDphWh4zvWeScnLkf')
    expect(SKUS['warden-2'].priceId).toBe('price_1TycKSEnDphWh4zvvE85TJf5')
    expect(SKUS['warden-3'].priceId).toBe('price_1TycKUEnDphWh4zvNSxOn2yI')
    expect(SKUS['warden-1'].productId).toBe('prod_UyZTENqYDCUtrt')
    expect(SKUS['warden-2'].productId).toBe('prod_UyZTw8O7rsyQjk')
    expect(SKUS['warden-3'].productId).toBe('prod_UyZToa7Y1nPVMG')
  })
})

describe('sku validation', () => {
  it.each(['warden-4', 'warden-0', '', 'WARDEN-1', 'warden-1 ', 'toString', '__proto__', 'constructor'])(
    'rejects %o',
    async (sku) => {
      const fake = createFakeStripe()
      const response = await handleCheckout(post(URL_, { sku }), asStripe(fake))
      expect(response.status).toBe(400)
      expect(await body(response)).toEqual({ error: 'unknown_sku' })
      expect(fake.calls.sessionCreate).toHaveLength(0)
    },
  )

  it('rejects a missing sku and a non-string sku', async () => {
    const fake = createFakeStripe()
    for (const payload of [{}, { sku: 1 }, { sku: null }, { sku: ['warden-1'] }, { sku: { id: 'warden-1' } }]) {
      const response = await handleCheckout(post(URL_, payload), asStripe(fake))
      expect(response.status).toBe(400)
    }
    expect(fake.calls.sessionCreate).toHaveLength(0)
  })

  it('rejects a body that is not JSON', async () => {
    const fake = createFakeStripe()
    const response = await handleCheckout(post(URL_, 'not json at all'), asStripe(fake))
    expect(response.status).toBe(400)
    expect(await body(response)).toEqual({ error: 'invalid_json' })
  })
})

describe('the session it creates', () => {
  const paramsFor = async (sku = 'warden-2') => {
    const fake = createFakeStripe()
    await handleCheckout(post(URL_, { sku }), asStripe(fake))
    return fake.calls.sessionCreate[0] as Record<string, any>
  }

  it('is a one-time payment, never a subscription', async () => {
    expect((await paramsFor()).mode).toBe('payment')
  })

  it('puts WARDEN on the card statement', async () => {
    expect((await paramsFor()).payment_intent_data.statement_descriptor_suffix).toBe('WARDEN')
  })

  it('sends the buyer to /success with the session id placeholder', async () => {
    expect((await paramsFor()).success_url).toBe('https://warden.test/success?session_id={CHECKOUT_SESSION_ID}')
  })

  it('cancels back to the pricing section', async () => {
    expect((await paramsFor()).cancel_url).toBe('https://warden.test/#pricing')
  })

  it('creates a Customer so /api/recover has something to search', async () => {
    expect((await paramsFor()).customer_creation).toBe('always')
  })

  it('returns the Stripe url and nothing else', async () => {
    const fake = createFakeStripe()
    const response = await handleCheckout(post(URL_, { sku: 'warden-1' }), asStripe(fake))
    const payload = await body(response)
    expect(Object.keys(payload)).toEqual(['url'])
    expect(payload.url).toMatch(/^https:\/\/checkout\.stripe\.com\//)
  })

  it('answers GET with 405', () => {
    expect(GET().status).toBe(405)
  })
})

describe('env override, so a different Stripe account is config not code', () => {
  it.each([
    ['warden-1', 'PRICE_WARDEN_1'],
    ['warden-2', 'PRICE_WARDEN_2'],
    ['warden-3', 'PRICE_WARDEN_3'],
  ])('uses %s from %s when it is set', async (sku, envVar) => {
    process.env[envVar] = 'price_from_the_other_account'
    const fake = createFakeStripe()

    const response = await handleCheckout(post(URL_, { sku }), asStripe(fake))
    expect(response.status).toBe(200)
    const params = fake.calls.sessionCreate[0] as { line_items: { price: string }[] }
    expect(params.line_items[0]?.price).toBe('price_from_the_other_account')
  })

  it('overrides one SKU without disturbing the others', async () => {
    process.env.PRICE_WARDEN_2 = 'price_override_only_two'
    expect(priceIdFor(SKUS['warden-2'])).toBe('price_override_only_two')
    expect(priceIdFor(SKUS['warden-1'])).toBe(SKUS['warden-1'].priceId)
    expect(priceIdFor(SKUS['warden-3'])).toBe(SKUS['warden-3'].priceId)
  })

  it('falls back to the committed default when the var is blank', async () => {
    process.env.PRICE_WARDEN_1 = '   '
    expect(priceIdFor(SKUS['warden-1'])).toBe('price_1TycKNEnDphWh4zvWeScnLkf')
  })

  it('a client cannot smuggle its own price id in', async () => {
    const fake = createFakeStripe()
    const response = await handleCheckout(
      post(URL_, { sku: 'warden-3', price: 'price_one_cent' }),
      asStripe(fake),
    )

    expect(response.status).toBe(400)
    expect(await body(response)).toEqual({ error: 'price_is_server_side' })
    expect(fake.calls.sessionCreate).toHaveLength(0)
  })
})
