import './env.js'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { HEAD, POST, handleDownload } from '../download.js'
import { issueToken } from '../_lib/tokens.js'
import { get } from './fake-stripe.js'

const URL_ = 'https://warden.test/api/download'
const SECRET = process.env.DOWNLOAD_TOKEN_SECRET as string
const NOW = Math.floor(Date.now() / 1000)
const ASSET_BYTES = Buffer.from('this pretends to be a disk image')

interface Call {
  url: string
  init: RequestInit | undefined
}

const makeFetch = (options: { redirect?: boolean; releaseStatus?: number; assets?: unknown[] } = {}) => {
  const calls: Call[] = []
  const impl = vi.fn(async (input: string | URL | Request, init?: RequestInit): Promise<Response> => {
    const url = String(input)
    calls.push({ url, init })

    // Asset URLs also contain "/releases/", so they are matched first.
    if (url.includes('/releases/assets/')) {
      if (options.redirect !== false) {
        return new Response(null, {
          status: 302,
          headers: { location: 'https://objects.githubusercontent.com/signed-blob' },
        })
      }
      return new Response(ASSET_BYTES, {
        status: 200,
        headers: { 'content-length': String(ASSET_BYTES.length) },
      })
    }

    if (url.includes('/releases/')) {
      if (options.releaseStatus && options.releaseStatus !== 200) {
        return new Response('{}', { status: options.releaseStatus })
      }
      return Response.json({
        tag_name: 'v0.1.0',
        assets: options.assets ?? [
          { url: 'https://api.github.com/repos/x/y/releases/assets/1', name: 'notes.txt', size: 4, content_type: 'text/plain' },
          {
            url: 'https://api.github.com/repos/x/y/releases/assets/2',
            name: 'WARDEN_0.1.0_aarch64.dmg',
            size: ASSET_BYTES.length,
            content_type: 'application/x-apple-diskimage',
          },
        ],
      })
    }

    return new Response(ASSET_BYTES, {
      status: 200,
      headers: { 'content-length': String(ASSET_BYTES.length) },
    })
  })

  return { impl: impl as unknown as typeof fetch, calls, spy: impl }
}

beforeEach(() => {
  delete process.env.GITHUB_RELEASE_TAG
})

describe('token validation', () => {
  it.each([
    ['no token', null],
    ['empty', ''],
    ['garbage', 'nonsense'],
    ['two segments', 'cs_x.999'],
    ['non-numeric expiry', 'cs_x.abc.aaaa'],
  ])('rejects %s with 403 and never touches GitHub', async (_label, token) => {
    const fetchImpl = makeFetch()
    const url = token === null ? URL_ : `${URL_}?token=${encodeURIComponent(token)}`

    const response = await handleDownload(get(url), fetchImpl.impl)
    expect(response.status).toBe(403)
    expect(await response.json()).toEqual({ error: 'invalid_token' })
    expect(fetchImpl.calls).toHaveLength(0)
  })

  it('rejects an expired token', async () => {
    const fetchImpl = makeFetch()
    const expired = issueToken('cs_expired', SECRET, NOW - 49 * 60 * 60)

    const response = await handleDownload(get(`${URL_}?token=${expired}`), fetchImpl.impl)
    expect(response.status).toBe(403)
    expect(fetchImpl.calls).toHaveLength(0)
  })

  it('rejects a token whose MAC was forged under another secret', async () => {
    const fetchImpl = makeFetch()
    const forged = issueToken('cs_forged', 'not-the-real-secret', NOW)

    const response = await handleDownload(get(`${URL_}?token=${forged}`), fetchImpl.impl)
    expect(response.status).toBe(403)
    expect(fetchImpl.calls).toHaveLength(0)
  })

  it('rejects an expiry stretched by hand', async () => {
    const fetchImpl = makeFetch()
    const [id, exp, mac] = issueToken('cs_stretch', SECRET, NOW).split('.') as [string, string, string]

    const response = await handleDownload(
      get(`${URL_}?token=${id}.${Number(exp) + 999_999}.${mac}`),
      fetchImpl.impl,
    )
    expect(response.status).toBe(403)
    expect(fetchImpl.calls).toHaveLength(0)
  })

  it('gives the same answer for expired, forged and malformed', async () => {
    const fetchImpl = makeFetch()
    const responses = await Promise.all([
      handleDownload(get(`${URL_}?token=${issueToken('cs_a', SECRET, NOW - 99999999)}`), fetchImpl.impl),
      handleDownload(get(`${URL_}?token=${issueToken('cs_a', 'wrong', NOW)}`), fetchImpl.impl),
      handleDownload(get(`${URL_}?token=junk`), fetchImpl.impl),
    ])
    const bodies = await Promise.all(responses.map((r) => r.text()))

    expect(new Set(responses.map((r) => r.status))).toEqual(new Set([403]))
    expect(new Set(bodies).size).toBe(1)
  })

  it('HEAD checks the token without fetching the asset', async () => {
    expect((await HEAD(get(`${URL_}?token=${issueToken('cs_h', SECRET, NOW)}`))).status).toBe(200)
    expect((await HEAD(get(`${URL_}?token=bad`))).status).toBe(403)
  })

  it('answers POST with 405', () => {
    expect(POST().status).toBe(405)
  })
})

describe('streaming the asset', () => {
  const validToken = () => issueToken('cs_valid', SECRET, NOW)

  it('streams the dmg back with a 200', async () => {
    const fetchImpl = makeFetch()
    const response = await handleDownload(get(`${URL_}?token=${validToken()}`), fetchImpl.impl)

    expect(response.status).toBe(200)
    expect(Buffer.from(await response.arrayBuffer())).toEqual(ASSET_BYTES)
    expect(response.headers.get('content-disposition')).toBe('attachment; filename="WARDEN_0.1.0_aarch64.dmg"')
    expect(response.headers.get('content-type')).toBe('application/x-apple-diskimage')
  })

  it('never redirects the client, so the token stays out of the browser hop', async () => {
    const fetchImpl = makeFetch()
    const response = await handleDownload(get(`${URL_}?token=${validToken()}`), fetchImpl.impl)

    expect(response.status).toBeLessThan(300)
    expect(response.headers.get('location')).toBeNull()
  })

  it('picks the dmg over other assets', async () => {
    const fetchImpl = makeFetch()
    await handleDownload(get(`${URL_}?token=${validToken()}`), fetchImpl.impl)
    expect(fetchImpl.calls[1]?.url).toBe('https://api.github.com/repos/x/y/releases/assets/2')
  })

  it('authenticates to GitHub but never forwards the PAT to the object store', async () => {
    const fetchImpl = makeFetch()
    await handleDownload(get(`${URL_}?token=${validToken()}`), fetchImpl.impl)

    const [release, asset, blob] = fetchImpl.calls as [Call, Call, Call]
    expect(new Headers(release.init?.headers).get('authorization')).toMatch(/^Bearer /)
    expect(new Headers(asset.init?.headers).get('authorization')).toMatch(/^Bearer /)
    expect(asset.init?.redirect).toBe('manual')

    expect(blob.url).toBe('https://objects.githubusercontent.com/signed-blob')
    expect(new Headers(blob.init?.headers).get('authorization')).toBeNull()
  })

  it('never leaks the download token to GitHub', async () => {
    const fetchImpl = makeFetch()
    const token = validToken()
    await handleDownload(get(`${URL_}?token=${token}`), fetchImpl.impl)

    const mac = token.split('.')[2] as string
    for (const call of fetchImpl.calls) {
      expect(call.url).not.toContain(mac)
      expect(JSON.stringify(call.init?.headers ?? {})).not.toContain(mac)
    }
  })

  it('handles an asset served without a redirect', async () => {
    const fetchImpl = makeFetch({ redirect: false })
    const response = await handleDownload(get(`${URL_}?token=${validToken()}`), fetchImpl.impl)

    expect(response.status).toBe(200)
    expect(Buffer.from(await response.arrayBuffer())).toEqual(ASSET_BYTES)
  })

  it('asks for a pinned tag when one is configured', async () => {
    process.env.GITHUB_RELEASE_TAG = 'v9.9.9'
    const fetchImpl = makeFetch()
    await handleDownload(get(`${URL_}?token=${validToken()}`), fetchImpl.impl)
    expect(fetchImpl.calls[0]?.url).toContain('/releases/tags/v9.9.9')
  })

  it('502s a GitHub outage without exposing the reason', async () => {
    const fetchImpl = makeFetch({ releaseStatus: 500 })
    const response = await handleDownload(get(`${URL_}?token=${validToken()}`), fetchImpl.impl)

    expect(response.status).toBe(502)
    expect(await response.json()).toEqual({ error: 'release_unavailable' })
  })

  it('404s a release with no assets', async () => {
    const fetchImpl = makeFetch({ assets: [] })
    const response = await handleDownload(get(`${URL_}?token=${validToken()}`), fetchImpl.impl)
    expect(response.status).toBe(404)
  })

  it('cannot be made to inject a response header through the asset name', async () => {
    const fetchImpl = makeFetch({
      assets: [
        {
          url: 'https://api.github.com/repos/x/y/releases/assets/9',
          name: 'evil".dmg\r\nset-cookie: a=b',
          size: 1,
          content_type: 'application/octet-stream',
        },
      ],
    })
    const response = await handleDownload(get(`${URL_}?token=${validToken()}`), fetchImpl.impl)

    expect(response.status).toBe(200)
    expect(response.headers.get('set-cookie')).toBeNull()
    expect(response.headers.get('content-disposition')).toBe('attachment; filename="evil_.dmg__set-cookie__a_b"')
  })
})
