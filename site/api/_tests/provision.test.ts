import './env.js'
import { afterEach, describe, expect, it, vi } from 'vitest'
// @ts-expect-error - a plain .mjs script with no type declarations
import { SKUS as SCRIPT_SKUS, CURRENCY, WEBHOOK_EVENTS, main } from '../../../scripts/stripe-provision.mjs'
import { SKUS, CURRENCY as API_CURRENCY } from '../_lib/stripe.js'

interface ScriptSku {
  id: string
  name: string
  description: string
  amount: number
  seats: number
}

const scriptSkus = SCRIPT_SKUS as ScriptSku[]

describe('the provisioning script cannot drift from the API price table', () => {
  it('lists the same SKUs', () => {
    expect(scriptSkus.map((sku) => sku.id)).toEqual(Object.keys(SKUS))
  })

  it.each(scriptSkus)('matches the served price and seats for $id', (sku) => {
    const api = SKUS[sku.id as keyof typeof SKUS]
    expect(api).toBeDefined()
    expect(sku.amount).toBe(api.amount)
    expect(sku.seats).toBe(api.seats)
    expect(sku.name).toBe(api.name)
    expect(sku.description).toBe(api.description)
  })

  it('charges in the same currency', () => {
    expect(CURRENCY).toBe(API_CURRENCY)
  })

  it('subscribes to the events the webhook actually fulfils', () => {
    expect(WEBHOOK_EVENTS).toContain('checkout.session.completed')
    expect(WEBHOOK_EVENTS).toContain('checkout.session.async_payment_succeeded')
  })
})

// The three products already exist live with Stripe-assigned ids, so the only
// thing standing between a re-run and a duplicate catalog is the metadata.sku
// match. That is worth simulating rather than trusting.
const LIVE_PRODUCTS = [
  {
    id: 'prod_UyZTENqYDCUtrt',
    name: 'WARDEN, one Mac',
    description:
      'WARDEN on a single Mac. A live board for every coding agent running on your machine. One payment, yours forever.',
    default_price: 'price_1TycKNEnDphWh4zvWeScnLkf',
    metadata: { sku: 'warden-1', seats: '1', product: 'warden' },
  },
  {
    id: 'prod_UyZTw8O7rsyQjk',
    name: 'WARDEN, two Macs',
    description:
      'WARDEN on two Macs. A live board for every coding agent running on your machines. One payment, yours forever.',
    default_price: 'price_1TycKSEnDphWh4zvvE85TJf5',
    metadata: { sku: 'warden-2', seats: '2', product: 'warden' },
  },
  {
    id: 'prod_UyZToa7Y1nPVMG',
    name: 'WARDEN, three Macs',
    description:
      'WARDEN on three Macs. A live board for every coding agent running on your machines. One payment, yours forever.',
    default_price: 'price_1TycKUEnDphWh4zvNSxOn2yI',
    metadata: { sku: 'warden-3', seats: '3', product: 'warden' },
  },
]

const LIVE_PRICES = [
  { id: 'price_1TycKNEnDphWh4zvWeScnLkf', product: 'prod_UyZTENqYDCUtrt', unit_amount: 1500, currency: 'usd', active: true },
  { id: 'price_1TycKSEnDphWh4zvvE85TJf5', product: 'prod_UyZTw8O7rsyQjk', unit_amount: 2500, currency: 'usd', active: true },
  { id: 'price_1TycKUEnDphWh4zvNSxOn2yI', product: 'prod_UyZToa7Y1nPVMG', unit_amount: 3000, currency: 'usd', active: true },
]

const WEBHOOK_URL = 'https://warden.test/api/webhook'
const LIVE_ENDPOINT = { id: 'we_live', url: WEBHOOK_URL, enabled_events: WEBHOOK_EVENTS }

/** A Stripe account simulated at the HTTP layer, so the script's real request
 *  building, pagination and matching all run. */
const fakeAccount = (seed: { products?: unknown[]; prices?: unknown[]; endpoints?: unknown[] } = {}) => {
  const products = structuredClone(seed.products ?? []) as Record<string, any>[]
  const prices = structuredClone(seed.prices ?? []) as Record<string, any>[]
  const endpoints = structuredClone(seed.endpoints ?? []) as Record<string, any>[]
  const writes: { method: string; path: string }[] = []
  let counter = 0

  const impl = vi.fn(async (input: string | URL | Request, init?: RequestInit): Promise<Response> => {
    const url = new URL(String(input))
    const path = url.pathname.replace('/v1', '')
    const method = init?.method ?? 'GET'
    const form = new URLSearchParams((init?.body as string) ?? '')

    if (method === 'GET') {
      if (path === '/products') return Response.json({ data: products, has_more: false })
      if (path === '/prices') {
        const product = url.searchParams.get('product')
        return Response.json({ data: prices.filter((p) => p.product === product && p.active), has_more: false })
      }
      if (path === '/webhook_endpoints') return Response.json({ data: endpoints, has_more: false })
      return Response.json({ error: { message: `unexpected GET ${path}` } }, { status: 404 })
    }

    writes.push({ method, path })
    counter += 1

    if (path === '/products') {
      const product = {
        id: `prod_new_${counter}`,
        name: form.get('name'),
        description: form.get('description'),
        default_price: null,
        metadata: { sku: form.get('metadata[sku]'), seats: form.get('metadata[seats]'), product: form.get('metadata[product]') },
      }
      products.push(product)
      return Response.json(product)
    }
    if (path.startsWith('/products/')) {
      const product = products.find((p) => p.id === path.split('/')[2])
      if (!product) return Response.json({ error: { message: 'no such product' } }, { status: 404 })
      if (form.get('name')) product.name = form.get('name')
      if (form.get('description')) product.description = form.get('description')
      if (form.get('default_price')) product.default_price = form.get('default_price')
      if (form.get('metadata[sku]')) {
        product.metadata = {
          sku: form.get('metadata[sku]'),
          seats: form.get('metadata[seats]'),
          product: form.get('metadata[product]'),
        }
      }
      return Response.json(product)
    }
    if (path === '/prices') {
      const price = {
        id: `price_new_${counter}`,
        product: form.get('product'),
        unit_amount: Number(form.get('unit_amount')),
        currency: form.get('currency'),
        active: true,
      }
      prices.push(price)
      return Response.json(price)
    }
    if (path === '/webhook_endpoints') {
      const endpoint = {
        id: `we_new_${counter}`,
        url: form.get('url'),
        enabled_events: form.getAll('enabled_events[]'),
        secret: 'whsec_freshly_minted',
      }
      endpoints.push(endpoint)
      return Response.json(endpoint)
    }
    if (path.startsWith('/webhook_endpoints/')) {
      const endpoint = endpoints.find((e) => e.id === path.split('/')[2])
      if (endpoint) endpoint.enabled_events = form.getAll('enabled_events[]')
      return Response.json(endpoint ?? {})
    }
    return Response.json({ error: { message: `unexpected ${method} ${path}` } }, { status: 404 })
  })

  return { impl, writes, products, prices, endpoints }
}

const provision = async (account: ReturnType<typeof fakeAccount>, argv = ['--url', WEBHOOK_URL]) => {
  const logs: string[] = []
  const log = console.log
  const realFetch = globalThis.fetch
  console.log = (...args: unknown[]) => void logs.push(args.join(' '))
  globalThis.fetch = account.impl as unknown as typeof fetch
  process.env.STRIPE_SECRET_KEY = 'sk_test_simulated'

  try {
    await (main as (argv: string[]) => Promise<void>)(argv)
    return logs.join('\n')
  } finally {
    console.log = log
    globalThis.fetch = realFetch
  }
}

afterEach(() => {
  delete process.env.STRIPE_SECRET_KEY
})

describe('idempotency against the account that already has these', () => {
  it('creates nothing when all three products and prices exist', async () => {
    const account = fakeAccount({
      products: LIVE_PRODUCTS,
      prices: LIVE_PRICES,
      endpoints: [{ id: 'we_live', url: WEBHOOK_URL, enabled_events: WEBHOOK_EVENTS }],
    })

    const output = await provision(account)

    expect(account.writes).toHaveLength(0)
    expect(account.products).toHaveLength(3)
    expect(account.prices).toHaveLength(3)
    expect(output).toContain('prod_UyZTENqYDCUtrt (exists, unchanged)')
    expect(output).toContain('price_1TycKNEnDphWh4zvWeScnLkf (exists, unchanged)')
  })

  it('stays at three products no matter how many times it runs', async () => {
    const account = fakeAccount({
      products: LIVE_PRODUCTS,
      prices: LIVE_PRICES,
      endpoints: [{ id: 'we_live', url: WEBHOOK_URL, enabled_events: WEBHOOK_EVENTS }],
    })

    for (let i = 0; i < 3; i += 1) await provision(account)

    expect(account.products).toHaveLength(3)
    expect(account.prices).toHaveLength(3)
    expect(account.endpoints).toHaveLength(1)
    expect(account.writes).toHaveLength(0)
  })

  it('prints the live price ids as PRICE_WARDEN_* env lines', async () => {
    const account = fakeAccount({ products: LIVE_PRODUCTS, prices: LIVE_PRICES, endpoints: [LIVE_ENDPOINT] })
    const output = await provision(account)

    expect(output).toContain('PRICE_WARDEN_1=price_1TycKNEnDphWh4zvWeScnLkf')
    expect(output).toContain('PRICE_WARDEN_2=price_1TycKSEnDphWh4zvvE85TJf5')
    expect(output).toContain('PRICE_WARDEN_3=price_1TycKUEnDphWh4zvNSxOn2yI')
  })

  it('matches on metadata.sku, not on the product name', async () => {
    // Renamed in the dashboard, and sharing the account with a similarly named
    // product from the other business. Only metadata.sku is load bearing.
    const renamed = structuredClone(LIVE_PRODUCTS)
    renamed[0]!.name = 'WARDEN (renamed by hand)'
    const decoy = {
      id: 'prod_frontier_decoy',
      name: 'WARDEN, one Mac',
      description: 'a different business selling something else',
      default_price: null,
      metadata: {},
    }

    const account = fakeAccount({ products: [decoy, ...renamed], prices: LIVE_PRICES, endpoints: [LIVE_ENDPOINT] })
    await provision(account)

    // It corrected the renamed one in place and left the decoy alone.
    expect(account.writes).toEqual([{ method: 'POST', path: '/products/prod_UyZTENqYDCUtrt' }])
    expect(account.products).toHaveLength(4)
    expect(account.products.find((p) => p.id === 'prod_UyZTENqYDCUtrt')?.name).toBe('WARDEN, one Mac')
    expect(account.products.find((p) => p.id === 'prod_frontier_decoy')?.name).toBe('WARDEN, one Mac')
  })

  it('updates drifted copy in place rather than creating a second product', async () => {
    const drifted = structuredClone(LIVE_PRODUCTS)
    drifted[1]!.description = 'stale description someone edited'
    delete (drifted[2]!.metadata as Record<string, string>).product

    const account = fakeAccount({ products: drifted, prices: LIVE_PRICES, endpoints: [LIVE_ENDPOINT] })
    await provision(account)

    expect(account.writes.every((w) => w.path.startsWith('/products/'))).toBe(true)
    expect(account.writes.some((w) => w.path === '/products')).toBe(false)
    expect(account.products).toHaveLength(3)
    expect(account.products[1]?.description).toBe(
      'WARDEN on two Macs. A live board for every coding agent running on your machines. One payment, yours forever.',
    )
  })

  it('adds a missing price to an existing product without touching the product', async () => {
    const account = fakeAccount({ products: LIVE_PRODUCTS, prices: LIVE_PRICES.slice(0, 2), endpoints: [LIVE_ENDPOINT] })
    await provision(account)

    // The product already has a default_price, so it is left untouched.
    expect(account.writes.map((w) => w.path)).toEqual(['/prices'])
    expect(account.prices).toHaveLength(3)
    expect(account.prices[2]).toMatchObject({ product: 'prod_UyZToa7Y1nPVMG', unit_amount: 3000, currency: 'usd' })
  })

  it('reports an extra active price rather than deleting it', async () => {
    const account = fakeAccount({
      products: LIVE_PRODUCTS,
      prices: [
        ...LIVE_PRICES,
        { id: 'price_old_launch', product: 'prod_UyZTENqYDCUtrt', unit_amount: 900, currency: 'usd', active: true },
      ],
      endpoints: [LIVE_ENDPOINT],
    })

    const output = await provision(account)
    expect(output).toContain('another active price on this product: price_old_launch')
    expect(account.writes).toHaveLength(0)
  })
})

describe('a fresh account', () => {
  it('builds the whole catalog once, then never again', async () => {
    const account = fakeAccount()

    const first = await provision(account)
    expect(account.products).toHaveLength(3)
    expect(account.prices).toHaveLength(3)
    expect(account.endpoints).toHaveLength(1)
    expect(first).toContain('whsec_freshly_minted')

    account.writes.length = 0
    await provision(account)

    expect(account.writes).toHaveLength(0)
    expect(account.products).toHaveLength(3)
    expect(account.prices).toHaveLength(3)
    expect(account.endpoints).toHaveLength(1)
  })

  it('sets each new product default_price and the right amounts', async () => {
    const account = fakeAccount()
    await provision(account)

    expect(account.prices.map((p) => p.unit_amount)).toEqual([1500, 2500, 3000])
    expect(account.products.every((p) => p.default_price)).toBe(true)
    expect(account.products.map((p) => p.metadata.sku)).toEqual(['warden-1', 'warden-2', 'warden-3'])
  })

  it('corrects a webhook endpoint that is subscribed to the wrong events', async () => {
    const account = fakeAccount({
      products: LIVE_PRODUCTS,
      prices: LIVE_PRICES,
      endpoints: [{ id: 'we_wrong', url: WEBHOOK_URL, enabled_events: ['payment_intent.created'] }],
    })

    const output = await provision(account)
    expect(account.writes).toEqual([{ method: 'POST', path: '/webhook_endpoints/we_wrong' }])
    expect(account.endpoints[0]?.enabled_events).toEqual(WEBHOOK_EVENTS)
    expect(output).toContain('(events updated)')
  })

  it('writes nothing at all under --dry-run', async () => {
    const account = fakeAccount()
    const output = await provision(account, ['--url', WEBHOOK_URL, '--dry-run'])

    expect(account.writes).toHaveLength(0)
    expect(account.products).toHaveLength(0)
    expect(output).toContain('DRY RUN')
  })
})

describe('it refuses to do damage by accident', () => {
  const run = async (argv: string[], env: Record<string, string | undefined>) => {
    const before = { ...process.env }
    const logs: string[] = []
    const errors: string[] = []
    const log = console.log
    const error = console.error
    console.log = (...args: unknown[]) => void logs.push(args.join(' '))
    console.error = (...args: unknown[]) => void errors.push(args.join(' '))
    const code = process.exitCode

    try {
      // Assigning undefined into process.env stores the STRING "undefined",
      // which is truthy: the script would sail past its own guard and make a
      // real call to Stripe. Unset means delete.
      for (const [key, value] of Object.entries(env)) {
        if (value === undefined) delete process.env[key]
        else process.env[key] = value
      }
      await (main as (argv: string[]) => Promise<void>)(argv)
      return { logs, errors, exitCode: process.exitCode }
    } finally {
      console.log = log
      console.error = error
      process.exitCode = code
      process.env = before
    }
  }

  it('does nothing without a key', async () => {
    const { errors, exitCode } = await run(['--url', 'https://warden.test/api/webhook'], {
      STRIPE_SECRET_KEY: undefined,
    })
    expect(errors.join()).toMatch(/STRIPE_SECRET_KEY is not set/)
    expect(exitCode).toBe(1)
  })

  it('demands a webhook url', async () => {
    const { errors, exitCode } = await run([], { STRIPE_SECRET_KEY: 'sk_test_x', WEBHOOK_URL: undefined })
    expect(errors.join()).toMatch(/webhook URL/)
    expect(exitCode).toBe(1)
  })

  it('refuses a non-https webhook url', async () => {
    const { errors, exitCode } = await run(['--url', 'http://warden.test/api/webhook'], {
      STRIPE_SECRET_KEY: 'sk_test_x',
    })
    expect(errors.join()).toMatch(/must be https/)
    expect(exitCode).toBe(1)
  })

  it('will not write to a live account without --yes', async () => {
    const { errors, exitCode } = await run(['--url', 'https://warden.test/api/webhook'], {
      STRIPE_SECRET_KEY: 'sk_live_notarealkey',
    })
    expect(errors.join()).toMatch(/LIVE key/)
    expect(exitCode).toBe(1)
  })

  it('never echoes the secret key', async () => {
    const { logs, errors } = await run(['--url', 'https://warden.test/api/webhook'], {
      STRIPE_SECRET_KEY: 'sk_live_notarealkey',
    })
    expect([...logs, ...errors].join('\n')).not.toContain('sk_live_notarealkey')
  })
})
