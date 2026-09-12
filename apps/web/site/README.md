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

- Coming-soon section (`#soon`): there is no sign-up form yet. When sign-ups
  open, add the form here and point the nav button at it.
- GitHub: the nav icon, the "Source" block, and the footer link all point at
  `#source`. Replace with the repository URL if the code is opened.
- X: the two links marked `title="Coming soon"` point at `#`.

The page uses the lowercase wordmark only; the well mark is deliberately not
shown on this page (owner direction, 2026-09-12).
