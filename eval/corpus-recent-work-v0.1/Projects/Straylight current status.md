# Straylight current status

Updated: 2026-07-26T12:00:00-07:00.

Current implementation truth:

- connection and authentication work;
- the owner corpus contains about 64,000 active records and 9.6 million
  estimated source tokens;
- exact-path reads work;
- broad `memory.open` and lexical retrieval are slow and can hit the 30-second
  database timeout;
- one successful retrieval lane can be lost when a later lane fails because
  the lanes share one transaction;
- normal writes copy a full corpus membership manifest;
- the current architecture is not ready for dependable owner use at 10x corpus
  growth.

Approved direction:

- make current Markdown files the workspace truth;
- version only changed files;
- make indexes and derived views rebuildable;
- checkpoint with exact path, version, and hash references plus changes since a
  workspace generation;
- retain coherent source hydration and compact retrieval packets.
