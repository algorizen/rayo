# Security Policy

## Supported versions

Pre-1.0: only the latest minor release receives security fixes. From 1.0: the latest minor of the current major, plus the final minor of the previous major for 12 months.

## Reporting a vulnerability

**Do not open a public issue for security vulnerabilities.**

Report privately via GitHub Security Advisories ("Report a vulnerability" on the repository) or email **security@algorizen.ai**. Include a description, reproduction steps, affected versions, and impact assessment if you have one.

We commit to:

- Acknowledgment within **48 hours**
- An initial assessment within **7 days**
- Coordinated disclosure: we'll agree on a timeline with you (default 90 days), credit you in the advisory unless you prefer otherwise, and publish a CVE where warranted

## Scope

In scope: the `rayo` package and all `rayo-*` crates — request parsing, validation, TLS configuration, the Rust↔Python boundary, job queue backends, and anything reachable from network input. Memory-safety issues in the Rust core and panics reachable from untrusted input are always in scope, even without a demonstrated exploit.

Out of scope: vulnerabilities in dependencies (report upstream — but tell us too so we can pin/patch), and issues requiring a hostile local user.

## Design commitments

- Untrusted input is parsed and validated in Rust before Python sees it; parser fuzzing runs in CI.
- No `unsafe` reachable from network input without a written safety argument (see [code standards](docs/code-standards.md)).
- Security-relevant defaults are safe: TLS via rustls, request-size and header limits on by default, no debug endpoints in prod mode.
