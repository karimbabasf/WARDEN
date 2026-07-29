import './env.js'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { createHmac, timingSafeEqual } from 'node:crypto'
import { describe, expect, it, vi } from 'vitest'
import { TOKEN_TTL_SECONDS, constantTimeEqual, issueToken, verifyToken } from '../_lib/tokens.js'

const SECRET = 'download_token_secret_for_unit_tests_only'
const OTHER_SECRET = 'a_different_secret_entirely'
const NOW = 1785700000
const ID = 'cs_test_download'

describe('issue', () => {
  it('produces <license_id>.<exp>.<mac> with a 48h expiry', () => {
    const token = issueToken(ID, SECRET, NOW)
    const [id, exp, mac] = token.split('.') as [string, string, string]
    expect(id).toBe(ID)
    expect(Number(exp)).toBe(NOW + TOKEN_TTL_SECONDS)
    expect(TOKEN_TTL_SECONDS).toBe(48 * 60 * 60)
    expect(Buffer.from(mac, 'base64url')).toHaveLength(32)
  })

  it('binds the MAC to both the id and the expiry', () => {
    const expected = createHmac('sha256', SECRET)
      .update(`${ID}:${NOW + TOKEN_TTL_SECONDS}`, 'utf8')
      .digest('base64url')
    expect(issueToken(ID, SECRET, NOW).split('.')[2]).toBe(expected)
  })

  it('refuses an id that is not a Stripe object id, so "." cannot be smuggled in', () => {
    expect(() => issueToken('cs.test.evil', SECRET, NOW)).toThrow(/Stripe object id/)
    expect(() => issueToken('', SECRET, NOW)).toThrow()
  })

  it('refuses to issue without a secret', () => {
    expect(() => issueToken(ID, '', NOW)).toThrow(/DOWNLOAD_TOKEN_SECRET/)
  })
})

describe('verify', () => {
  it('accepts a fresh token', () => {
    expect(verifyToken(issueToken(ID, SECRET, NOW), SECRET, NOW + 60)).toEqual({
      licenseId: ID,
      exp: NOW + TOKEN_TTL_SECONDS,
    })
  })

  it('rejects an expired token', () => {
    const token = issueToken(ID, SECRET, NOW)
    expect(verifyToken(token, SECRET, NOW + TOKEN_TTL_SECONDS - 1)).not.toBeNull()
    expect(verifyToken(token, SECRET, NOW + TOKEN_TTL_SECONDS)).toBeNull()
    expect(verifyToken(token, SECRET, NOW + TOKEN_TTL_SECONDS + 1)).toBeNull()
  })

  it('rejects a wrong MAC', () => {
    const [id, exp] = issueToken(ID, SECRET, NOW).split('.') as [string, string]
    const forged = createHmac('sha256', OTHER_SECRET).update(`${id}:${exp}`).digest('base64url')
    expect(verifyToken(`${id}.${exp}.${forged}`, SECRET, NOW)).toBeNull()
  })

  it('rejects a token minted under a different secret', () => {
    expect(verifyToken(issueToken(ID, OTHER_SECRET, NOW), SECRET, NOW)).toBeNull()
  })

  it('rejects an extended expiry, the whole point of covering exp with the MAC', () => {
    const [id, exp, mac] = issueToken(ID, SECRET, NOW).split('.') as [string, string, string]
    const stretched = `${id}.${Number(exp) + 86_400 * 365}.${mac}`
    expect(verifyToken(stretched, SECRET, NOW)).toBeNull()
  })

  it('rejects a swapped license id at a valid MAC', () => {
    const [, exp, mac] = issueToken(ID, SECRET, NOW).split('.') as [string, string, string]
    expect(verifyToken(`cs_test_someoneelse.${exp}.${mac}`, SECRET, NOW)).toBeNull()
  })

  it('rejects every single-bit flip in the MAC', () => {
    const [id, exp, mac] = issueToken(ID, SECRET, NOW).split('.') as [string, string, string]
    const bytes = Buffer.from(mac, 'base64url')
    for (const i of [0, 15, 31]) {
      const flipped = Buffer.from(bytes)
      flipped.writeUInt8(flipped.readUInt8(i) ^ 0x01, i)
      expect(verifyToken(`${id}.${exp}.${flipped.toString('base64url')}`, SECRET, NOW)).toBeNull()
    }
  })

  it('rejects malformed shapes without throwing', () => {
    for (const token of [
      '',
      'nonsense',
      `${ID}.${NOW}`,
      `${ID}.${NOW}.mac.extra`,
      `${ID}..mac`,
      `${ID}.notanumber.${'a'.repeat(43)}`,
      `${ID}.-1.${'a'.repeat(43)}`,
      `${ID}.${NOW}.`,
      null,
      undefined,
    ]) {
      expect(verifyToken(token as string, SECRET, NOW)).toBeNull()
    }
  })

  it('rejects a MAC of the wrong length rather than throwing in timingSafeEqual', () => {
    const [id, exp, mac] = issueToken(ID, SECRET, NOW).split('.') as [string, string, string]
    const short = Buffer.from(mac, 'base64url').subarray(0, 16).toString('base64url')
    expect(() => verifyToken(`${id}.${exp}.${short}`, SECRET, NOW)).not.toThrow()
    expect(verifyToken(`${id}.${exp}.${short}`, SECRET, NOW)).toBeNull()
  })

  it('rejects everything when the server has no secret', () => {
    expect(verifyToken(issueToken(ID, SECRET, NOW), '', NOW)).toBeNull()
  })
})

describe('constant-time comparison', () => {
  it('constantTimeEqual delegates to crypto.timingSafeEqual', () => {
    const a = Buffer.alloc(32, 1)
    const b = Buffer.alloc(32, 1)
    expect(constantTimeEqual(a, b)).toBe(true)
    expect(constantTimeEqual(a, Buffer.alloc(32, 2))).toBe(false)
    // A length mismatch would throw inside timingSafeEqual if it were called
    // unguarded, so this both proves the guard and that no exception escapes.
    expect(constantTimeEqual(a, Buffer.alloc(16, 1))).toBe(false)
    expect(timingSafeEqual(a, b)).toBe(true)
  })

  it('verifyToken actually calls timingSafeEqual on the MAC', async () => {
    vi.resetModules()
    const spy = vi.fn(timingSafeEqual)
    vi.doMock('node:crypto', async () => {
      const actual = await vi.importActual<typeof import('node:crypto')>('node:crypto')
      return { ...actual, default: actual, timingSafeEqual: spy }
    })

    const tokens = await import('../_lib/tokens.js')
    const token = tokens.issueToken(ID, SECRET, NOW)
    expect(spy).not.toHaveBeenCalled()

    expect(tokens.verifyToken(token, SECRET, NOW)).not.toBeNull()
    expect(spy).toHaveBeenCalledTimes(1)
    const [provided, expected] = spy.mock.calls[0] as [Buffer, Buffer]
    expect(provided).toHaveLength(32)
    expect(expected).toHaveLength(32)

    vi.doUnmock('node:crypto')
    vi.resetModules()
  })

  it('never compares the MAC with a variable-time string equality', () => {
    const source = readFileSync(join(import.meta.dirname, '../_lib/tokens.ts'), 'utf8')
    const body = source
      .split('\n')
      .filter((line) => !line.trimStart().startsWith('//') && !line.trimStart().startsWith('*'))
      .join('\n')
    expect(body).toContain('timingSafeEqual')
    // The only == / === allowed near the MAC is the length guard.
    expect(body).not.toMatch(/mac\w*\s*={2,3}/i)
    expect(body).not.toMatch(/provided\s*={2,3}\s*/)
  })
})
