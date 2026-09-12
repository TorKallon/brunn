# brunn front page

`index.html` is the public marketing page served at `https://brunn.ai/`.
It is a single static document with no build step, no JavaScript, no
external requests, and inline Still Water tokens from `docs/Brand.md`, so it
passes the control-plane Content Security Policy unchanged.

Routing: nginx serves `/site/index.html` for the exact path `/` (see
`nginx.conf` and `nginx.railway.conf.template`). Every other path still
resolves to the SPA, so `/login` and the app are unaffected. Both Dockerfiles
copy this directory to `/usr/share/nginx/html/site`.

SEO companions live in `public/`: `robots.txt`, `sitemap.xml`, and the
existing `og.png`. Structured data (Organization, SoftwareApplication,
FAQPage) is inline in the page head.

Placeholders to fill in before launch:

- Early-access form (`#soon`) posts to `/waitlist`, a Cloudflare Worker in
  `infra/cloudflare/waitlist/`. The Worker redirects to `/#joined` or
  `/#retry`; the page shows those messages with CSS `:target`.
- GitHub: the nav icon, the "Source" block, and the footer link all point at
  `#source`. Replace with the repository URL if the code is opened.
- X: no links yet; add them when the account exists.
- `privacy.html` is served at `/privacy` and names hello@brunn.ai, which
  Cloudflare Email Routing forwards to the owner.

The page uses the lowercase wordmark only; the well mark is deliberately not
shown on this page (owner direction, 2026-09-12).
