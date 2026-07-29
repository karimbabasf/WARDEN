// Download tokens. HMAC-SHA256 over `<license_id>:<exp>` with
// DOWNLOAD_TOKEN_SECRET, format `<license_id>.<exp>.<base64url(mac)>`, 48h life.
//
// This is anti-hotlink, not DRM (see LAUNCH-SPEC section 3). The product gate is
// the license key; the token only stops the release asset being scraped, which
// is why it can be stateless and short-lived without anyone losing what they
// bought. It is still compared in constant time, because a MAC that leaks
// through timing is not a MAC.

import { createHmac, timingSafeEqual } from 'node:crypto'

export const TOKEN_TTL_SECONDS = 48 * 60 * 60
const MAC_BYTES = 32

/** Stripe object ids are `[A-Za-z0-9_]+`. Constraining the id to that charset is
 *  what makes splitting the token on "." unambiguous, and it stops a crafted id
 *  from smuggling a second field into the MAC input. */
const LICENSE_ID = /^[A-Za-z0-9_]+$/
const DIGITS = /^[0-9]{1,15}$/

export interface DownloadToken {
  licenseId: string
  exp: number
}

const macFor = (licenseId: string, exp: number, secret: string): Buffer =>
  createHmac('sha256', secret).update(`${licenseId}:${exp}`, 'utf8').digest()

/** Constant-time buffer compare. timingSafeEqual throws on a length mismatch, so
 *  the length is checked first; MAC length is fixed and public, so that check
 *  leaks nothing an attacker did not already know. */
export const constantTimeEqual = (a: Buffer, b: Buffer): boolean =>
  a.length === b.length && timingSafeEqual(a, b)

export const issueToken = (
  licenseId: string,
  secret: string,
  now: number = Math.floor(Date.now() / 1000),
  ttl: number = TOKEN_TTL_SECONDS,
): string => {
  if (!LICENSE_ID.test(licenseId)) throw new Error('license id is not a Stripe object id')
  if (!secret) throw new Error('DOWNLOAD_TOKEN_SECRET is not set')
  const exp = now + ttl
  return `${licenseId}.${exp}.${macFor(licenseId, exp, secret).toString('base64url')}`
}

/**
 * Returns the token's claims, or null. The MAC is checked BEFORE the expiry, so
 * `exp` is authenticated before it is believed: checking expiry first would let
 * anyone rewrite the expiry of a token they cannot forge and learn, from the
 * different rejection, whether the rest of it was valid.
 */
export const verifyToken = (
  token: string | null | undefined,
  secret: string,
  now: number = Math.floor(Date.now() / 1000),
): DownloadToken | null => {
  if (typeof token !== 'string' || !secret) return null

  const parts = token.split('.')
  if (parts.length !== 3) return null
  const [licenseId, expRaw, macSeg] = parts as [string, string, string]

  if (!LICENSE_ID.test(licenseId) || !DIGITS.test(expRaw)) return null
  const exp = Number(expRaw)
  if (!Number.isSafeInteger(exp)) return null

  const provided = Buffer.from(macSeg, 'base64url')
  if (provided.length !== MAC_BYTES) return null
  if (!constantTimeEqual(provided, macFor(licenseId, exp, secret))) return null

  if (exp <= now) return null
  return { licenseId, exp }
}
