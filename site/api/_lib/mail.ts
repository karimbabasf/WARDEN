// Receipt email via Resend, over plain fetch so this adds no dependency.
//
// Email is a convenience copy, never the critical path (LAUNCH-SPEC section 1):
// the success page renders the key and the download button itself. So nothing
// here throws. A missing RESEND_API_KEY logs a warning and the purchase still
// succeeds; a Resend outage must not turn a paid order into a failed one.

const ENDPOINT = 'https://api.resend.com/emails'
const DEFAULT_FROM = 'WARDEN <keys@warden.app>'

export type MailResult = 'sent' | 'skipped' | 'failed'

export interface LicenseMail {
  to: string
  licenseKey: string
  seats: number
  downloadUrl: string
}

const escapeHtml = (value: string): string =>
  value.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c] as string)

const plainBody = ({ licenseKey, seats, downloadUrl }: LicenseMail): string =>
  [
    'Thanks for buying WARDEN.',
    '',
    `Your license key (${seats} ${seats === 1 ? 'Mac' : 'Macs'}):`,
    '',
    licenseKey,
    '',
    'Download the app (link expires in 48 hours):',
    downloadUrl,
    '',
    'Paste the key into WARDEN on first launch. It is checked offline, so the app',
    'never phones home and the key keeps working without an internet connection.',
    '',
    'Lost the key? Ask for it again at the recover link on the site, using the',
    'email address you paid with.',
  ].join('\n')

const htmlBody = (mail: LicenseMail): string => {
  const key = escapeHtml(mail.licenseKey)
  const url = escapeHtml(mail.downloadUrl)
  return [
    '<div style="font-family:ui-sans-serif,system-ui,-apple-system,sans-serif;line-height:1.55;color:#111">',
    '<p>Thanks for buying WARDEN.</p>',
    `<p>Your license key (${mail.seats} ${mail.seats === 1 ? 'Mac' : 'Macs'}):</p>`,
    `<pre style="background:#f4f5f7;padding:14px;border-radius:8px;white-space:pre-wrap;word-break:break-all;font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:13px">${key}</pre>`,
    `<p><a href="${url}" style="display:inline-block;background:#111;color:#fff;padding:11px 18px;border-radius:8px;text-decoration:none">Download WARDEN for macOS</a></p>`,
    '<p style="color:#666;font-size:13px">That download link expires in 48 hours. Your license key does not expire.</p>',
    '<p style="color:#666;font-size:13px">Paste the key into WARDEN on first launch. It is checked offline, so the app never phones home.</p>',
    '</div>',
  ].join('')
}

/**
 * Sends the license email. Returns rather than throws, always.
 *
 * The recipient address is never logged: it is customer data, and a log line is
 * the easiest place in the system to leak it from.
 */
export const sendLicenseEmail = async (mail: LicenseMail): Promise<MailResult> => {
  const apiKey = process.env.RESEND_API_KEY
  if (!apiKey) {
    console.warn('[mail] RESEND_API_KEY is not set, skipping the receipt email')
    return 'skipped'
  }

  try {
    const response = await fetch(ENDPOINT, {
      method: 'POST',
      headers: { authorization: `Bearer ${apiKey}`, 'content-type': 'application/json' },
      body: JSON.stringify({
        from: process.env.LICENSE_EMAIL_FROM?.trim() || DEFAULT_FROM,
        to: [mail.to],
        subject: 'Your WARDEN license key',
        text: plainBody(mail),
        html: htmlBody(mail),
      }),
    })
    if (!response.ok) {
      console.error(`[mail] resend rejected the send: ${response.status}`)
      return 'failed'
    }
    return 'sent'
  } catch (err) {
    console.error(`[mail] send failed: ${err instanceof Error ? err.message : 'unknown error'}`)
    return 'failed'
  }
}
