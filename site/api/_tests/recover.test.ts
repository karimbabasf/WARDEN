import './env.js'
import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('../_lib/mail.js', () => ({ sendLicenseEmail: vi.fn(async () => 'sent' as const) }))

import { GET, __resetRateLimit, handleRecover } from '../recover.js'
import { sendLicenseEmail } from '../_lib/mail.js'
import { asStripe, createFakeStripe, post } from './fake-stripe.js'

const URL_ = 'https://warden.test/api/recover'
const mail = vi.mocked(sendLicenseEmail)
const body = async (response: Response) => (await response.json()) as Record<string, unknown>

const withBuyer = () =>
  createFakeStripe([
    { id: 'cs_bought', email: 'buyer@example.com', customer: 'cus_1', metadata: { sku: 'warden-2' } },
  ]).withCustomers([{ id: 'cus_1', email: 'buyer@example.com' }], { cus_1: ['cs_bought'] })

const request = (email: unknown, ip = '203.0.113.1') =>
  post(URL_, { email }, { 'x-forwarded-for': ip })

beforeEach(() => {
  __resetRateLimit()
  mail.mockReset()
  mail.mockResolvedValue('sent')
})

describe('it never reveals whether an address bought anything', () => {
  it('answers a known buyer and a stranger identically', async () => {
    const known = await handleRecover(request('buyer@example.com'), asStripe(withBuyer()))
    __resetRateLimit()
    const unknown = await handleRecover(request('nobody@example.com'), asStripe(withBuyer()))

    expect(known.status).toBe(unknown.status)
    expect(await known.text()).toBe(await unknown.text())
    expect(known.status).toBe(200)
  })

  it('answers a malformed address the same way too', async () => {
    const fake = withBuyer()
    for (const email of ['', 'not-an-email', 'a@b', '@example.com', 'x'.repeat(400), 'a b@example.com']) {
      const response = await handleRecover(request(email), asStripe(fake))
      expect(response.status).toBe(200)
      expect(await body(response)).toEqual({ ok: true })
    }
    expect(fake.calls.customerList).toHaveLength(0)
    expect(mail).not.toHaveBeenCalled()
  })

  it('answers a non-string email the same way', async () => {
    const fake = withBuyer()
    for (const email of [null, 42, { email: 'a@b.co' }, ['a@b.co']]) {
      expect((await handleRecover(request(email), asStripe(fake))).status).toBe(200)
    }
    expect(mail).not.toHaveBeenCalled()
  })

  it('stays ok:true when Stripe itself fails', async () => {
    const fake = withBuyer()
    fake.customers.list = async () => {
      throw new Error('stripe is down')
    }

    const response = await handleRecover(request('buyer@example.com'), asStripe(fake))
    expect(response.status).toBe(200)
    expect(await body(response)).toEqual({ ok: true })
  })

  it('rejects a body that is not JSON', async () => {
    const response = await handleRecover(post(URL_, 'nope'), asStripe(withBuyer()))
    expect(response.status).toBe(400)
  })
})

describe('resending', () => {
  it('emails the key of a paid session', async () => {
    const fake = withBuyer()
    await handleRecover(request('buyer@example.com'), asStripe(fake))

    expect(mail).toHaveBeenCalledTimes(1)
    const sent = mail.mock.calls[0]?.[0]
    expect(sent?.to).toBe('buyer@example.com')
    expect(sent?.seats).toBe(2)
    expect(sent?.licenseKey).toMatch(/^WRDN-/)
    expect(sent?.downloadUrl).toContain('/api/download?token=cs_bought.')
  })

  it('sends nothing for an address with no purchases', async () => {
    await handleRecover(request('stranger@example.com'), asStripe(withBuyer()))
    expect(mail).not.toHaveBeenCalled()
  })

  it('skips sessions that were never paid', async () => {
    const fake = createFakeStripe([
      { id: 'cs_abandoned', email: 'buyer@example.com', customer: 'cus_1', payment_status: 'unpaid' },
    ]).withCustomers([{ id: 'cus_1', email: 'buyer@example.com' }], { cus_1: ['cs_abandoned'] })

    await handleRecover(request('buyer@example.com'), asStripe(fake))
    expect(mail).not.toHaveBeenCalled()
  })

  it('still finds the WARDEN key when the buyer also bought from the shared account', async () => {
    const fake = createFakeStripe([
      { id: 'cs_frontier', email: 'buyer@example.com', customer: 'cus_1', metadata: { sku: 'frontier-thing' } },
      { id: 'cs_warden', email: 'buyer@example.com', customer: 'cus_1', metadata: { sku: 'warden-3' } },
    ]).withCustomers([{ id: 'cus_1', email: 'buyer@example.com' }], { cus_1: ['cs_frontier', 'cs_warden'] })

    await handleRecover(request('buyer@example.com'), asStripe(fake))

    expect(mail).toHaveBeenCalledTimes(1)
    expect(mail.mock.calls[0]?.[0].seats).toBe(3)
  })

  it('mints for a paid session the webhook never fulfilled', async () => {
    const fake = withBuyer()
    await handleRecover(request('buyer@example.com'), asStripe(fake))
    expect(fake.sessions.get('cs_bought')?.metadata.license_key).toMatch(/^WRDN-/)
  })

  it('looks the address up in the case it was typed as well as lowercased', async () => {
    const fake = createFakeStripe([
      { id: 'cs_mixed', email: 'Buyer@Example.com', customer: 'cus_9' },
    ]).withCustomers([{ id: 'cus_9', email: 'Buyer@Example.com' }], { cus_9: ['cs_mixed'] })

    await handleRecover(request('Buyer@Example.com'), asStripe(fake))
    expect(fake.calls.customerList).toEqual([
      { email: 'buyer@example.com', limit: 10 },
      { email: 'Buyer@Example.com', limit: 10 },
    ])
    expect(mail).toHaveBeenCalledTimes(1)
  })

  it('does not send the same key twice', async () => {
    const fake = createFakeStripe([
      { id: 'cs_one', email: 'buyer@example.com', customer: 'cus_1' },
    ]).withCustomers(
      [
        { id: 'cus_1', email: 'buyer@example.com' },
        { id: 'cus_2', email: 'buyer@example.com' },
      ],
      { cus_1: ['cs_one'], cus_2: ['cs_one'] },
    )

    await handleRecover(request('buyer@example.com'), asStripe(fake))
    expect(mail).toHaveBeenCalledTimes(1)
  })
})

describe('rate limiting', () => {
  it('cuts an IP off after 5 attempts in the window', async () => {
    const fake = withBuyer()
    const statuses: number[] = []
    for (let i = 0; i < 7; i += 1) {
      statuses.push((await handleRecover(request(`user${i}@example.com`, '198.51.100.7'), asStripe(fake))).status)
    }
    expect(statuses).toEqual([200, 200, 200, 200, 200, 429, 429])
  })

  it('tells a rate-limited caller when to come back', async () => {
    const fake = withBuyer()
    const start = Date.now()
    let limited: Response | undefined
    for (let i = 0; i < 6; i += 1) {
      limited = await handleRecover(request(`u${i}@example.com`, '198.51.100.30'), asStripe(fake), start)
    }

    expect(limited?.status).toBe(429)
    expect(await limited?.json()).toEqual({ error: 'rate_limited' })
    // The window is 15 minutes and no time has passed in this loop.
    expect(Number(limited?.headers.get('retry-after'))).toBe(15 * 60)
  })

  it('counts down Retry-After as the window elapses', async () => {
    const fake = withBuyer()
    const start = Date.now()
    for (let i = 0; i < 5; i += 1) {
      await handleRecover(request(`u${i}@example.com`, '198.51.100.31'), asStripe(fake), start)
    }

    const later = await handleRecover(
      request('u9@example.com', '198.51.100.31'),
      asStripe(fake),
      start + 10 * 60 * 1000,
    )
    expect(later.status).toBe(429)
    expect(Number(later.headers.get('retry-after'))).toBe(5 * 60)
  })

  it('never sets Retry-After on the silent per-address cap', async () => {
    const fake = withBuyer()
    let last: Response | undefined
    for (let i = 0; i < 4; i += 1) {
      last = await handleRecover(request('buyer@example.com', `192.0.2.${100 + i}`), asStripe(fake))
    }

    expect(last?.status).toBe(200)
    expect(last?.headers.get('retry-after')).toBeNull()
  })

  it('lets a different IP through', async () => {
    const fake = withBuyer()
    for (let i = 0; i < 6; i += 1) await handleRecover(request('a@example.com', '198.51.100.8'), asStripe(fake))

    expect((await handleRecover(request('a@example.com', '198.51.100.9'), asStripe(fake))).status).toBe(200)
  })

  it('caps repeat requests for one address silently, without a 429', async () => {
    const fake = withBuyer()
    const responses: Response[] = []
    for (let i = 0; i < 4; i += 1) {
      responses.push(await handleRecover(request('buyer@example.com', `192.0.2.${i}`), asStripe(fake)))
    }

    expect(responses.map((r) => r.status)).toEqual([200, 200, 200, 200])
    // The 4th crossed MAX_PER_EMAIL, so only the first three sent mail. A 429
    // there would have said "someone asked about this address recently".
    expect(mail).toHaveBeenCalledTimes(3)
  })

  it('reopens the window once it has passed', async () => {
    const fake = withBuyer()
    const start = Date.now()
    for (let i = 0; i < 6; i += 1) await handleRecover(request('a@example.com', '198.51.100.20'), asStripe(fake), start)

    const later = await handleRecover(request('a@example.com', '198.51.100.20'), asStripe(fake), start + 16 * 60 * 1000)
    expect(later.status).toBe(200)
  })

  it('answers GET with 405', () => {
    expect(GET().status).toBe(405)
  })
})
