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
