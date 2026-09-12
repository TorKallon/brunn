// brunn.ai/waitlist — one-field email capture.
// Cloudflare Worker bound to KV namespace WAITLIST. No JavaScript on the page:
// the form POSTs here and the Worker redirects back to a fragment the page
// reveals with CSS (:target). GET redirects to the form.

const EMAIL = /^[^\s@]{1,64}@[^\s@]+\.[^\s@]{2,}$/;
const PER_IP_PER_HOUR = 5;

function back(url, fragment) {
  return Response.redirect(`${url.origin}/#${fragment}`, 303);
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (request.method !== "POST") return back(url, "soon");

    let form;
    try {
      form = await request.formData();
    } catch {
      return back(url, "retry");
    }
    const email = String(form.get("email") ?? "").trim().toLowerCase();
    const trap = String(form.get("website") ?? "");
    // Honeypot filled in: a bot. Pretend it worked and store nothing.
    if (trap) return back(url, "joined");
    if (email.length > 254 || !EMAIL.test(email)) return back(url, "retry");

    const ip = request.headers.get("cf-connecting-ip") ?? "unknown";
    const rateKey = `rate:${ip}`;
    const seen = Number((await env.WAITLIST.get(rateKey)) ?? "0");
    if (seen >= PER_IP_PER_HOUR) return back(url, "joined");
    await env.WAITLIST.put(rateKey, String(seen + 1), { expirationTtl: 3600 });

    const key = `email:${email}`;
    if (!(await env.WAITLIST.get(key))) {
      await env.WAITLIST.put(
        key,
        JSON.stringify({
          email,
          joined_at: new Date().toISOString(),
          country: request.cf?.country ?? null,
          referer: request.headers.get("referer") ?? null,
        }),
      );
    }
    return back(url, "joined");
  },
};
