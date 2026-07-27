# Durable note authoring contract

When a new Markdown note factually corrects an existing workspace note, preserve
both sources and declare the exact older vault-relative path in the new note:

```yaml
---
supersedes:
  - exact/older/path.md
---
```

When a user creates a future obligation, write an ordinary note whose
frontmatter records `kind: intention`, a short `trigger` list, an ISO date in
`due`, and `status: pending`. When the obligation is complete, edit that same
note to `status: done`. Keep the body source-bearing; the frontmatter is the
portable, rebuildable declaration.
